//! Per-connection protocol state: role negotiation at `hello`, the fixed
//! six-method surface, and the capability cap that enforces constitutional
//! rule 6 at the boundary — an agent-role connection can never invoke an
//! effect above `propose`, checked *before* dispatch, so the command layer
//! never even sees the call.

use std::collections::HashSet;
use std::sync::Arc;

use compos_core::Effect;
use serde_json::{Value, json};

use crate::AppState;
use crate::events::{Event, TOPICS};
use crate::proto::{self, Request, WireError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Shell,
    Service,
    Agent,
    Maintenance,
}

impl Role {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "shell" => Some(Role::Shell),
            "service" => Some(Role::Service),
            "agent" => Some(Role::Agent),
            "maintenance" => Some(Role::Maintenance),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Role::Shell => "shell",
            Role::Service => "service",
            Role::Agent => "agent",
            Role::Maintenance => "maintenance",
        }
    }

    /// The §6 capability cap. Shell and service top out at `commit`;
    /// agents at `propose`; only maintenance reaches `system`.
    fn max_effect(self) -> Effect {
        match self {
            Role::Shell | Role::Service => Effect::Commit,
            Role::Agent => Effect::Propose,
            Role::Maintenance => Effect::System,
        }
    }
}

/// How this connection arrived, deciding its authentication rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Peer credentials were verified at accept (same UID) — the dev-profile
    /// convention of §6; no token needed.
    Uds,
    /// Localhost WebSocket: the startup token is required at `hello`.
    WebSocket,
}

pub struct Session {
    state: Arc<AppState>,
    transport: Transport,
    role: Option<Role>,
    topics: HashSet<String>,
}

impl Session {
    pub fn new(state: Arc<AppState>, transport: Transport) -> Self {
        Self {
            state,
            transport,
            role: None,
            topics: HashSet::new(),
        }
    }

    /// Whether this connection should receive events on `topic`.
    pub fn wants(&self, event: &Event) -> bool {
        self.role.is_some() && self.topics.contains(&event.topic)
    }

    /// Handle one raw JSON-RPC message; `None` means no response is due
    /// (notification or unparseable id).
    pub fn handle(&mut self, raw: &str) -> Option<String> {
        let req: Request = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                return Some(proto::response_err(
                    &Value::Null,
                    &WireError::new(proto::PARSE_ERROR, "PARSE_ERROR", e.to_string()),
                ));
            }
        };
        let id = req.id.clone()?; // notification: nothing to answer
        let result = self.dispatch(&req);
        Some(match result {
            Ok(value) => proto::response_ok(&id, value),
            Err(err) => proto::response_err(&id, &err),
        })
    }

    fn dispatch(&mut self, req: &Request) -> Result<Value, WireError> {
        match req.method.as_str() {
            "hello" => self.hello(&req.params),
            "commands.list" => self
                .authed()
                .map(|_| json!({"commands": self.state.registry.list()})),
            "commands.describe" => {
                self.authed()?;
                let id = req
                    .params
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        WireError::validation_failed("'command' (string) is required")
                    })?;
                match self.state.registry.describe(id) {
                    Some(spec) => Ok(json!({"command": spec})),
                    None => Err(WireError::new(
                        proto::COMMAND_UNKNOWN,
                        "COMMAND_UNKNOWN",
                        format!("unknown command: {id}"),
                    )),
                }
            }
            "commands.invoke" => self.invoke(&req.params),
            "jobs.cancel" => {
                self.authed()?;
                // Phase 1 has no long-running invokes yet, so every job id
                // is unknown by definition.
                Err(WireError::new(
                    proto::JOB_UNKNOWN,
                    "JOB_UNKNOWN",
                    "no such job (no long-running commands in this build)",
                ))
            }
            "events.subscribe" => self.subscribe(&req.params),
            other => Err(WireError::new(
                proto::METHOD_NOT_FOUND,
                "METHOD_NOT_FOUND",
                format!("unknown method: {other}"),
            )),
        }
    }

    fn authed(&self) -> Result<Role, WireError> {
        self.role
            .ok_or_else(|| WireError::capability_denied("say hello first"))
    }

    fn hello(&mut self, params: &Value) -> Result<Value, WireError> {
        let requested = params
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| WireError::validation_failed("'role' (string) is required"))?;
        let role = Role::parse(requested).ok_or_else(|| {
            WireError::validation_failed(format!(
                "unknown role '{requested}' (shell|service|agent|maintenance)"
            ))
        })?;

        if self.transport == Transport::WebSocket {
            let token = params.get("token").and_then(Value::as_str).unwrap_or("");
            if token.is_empty() || token != self.state.token {
                return Err(WireError::capability_denied("bad or missing token"));
            }
        }

        self.role = Some(role);
        Ok(json!({
            "protocol": proto::PROTOCOL_VERSION,
            "role_granted": role.as_str(),
            "capabilities": [format!("effect:{}", role.max_effect())],
        }))
    }

    fn invoke(&mut self, params: &Value) -> Result<Value, WireError> {
        let role = self.authed()?;
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| WireError::validation_failed("'command' (string) is required"))?;
        let input = params.get("input").cloned().unwrap_or(json!({}));

        // The §6 boundary check: role cap versus declared effect, before
        // anything runs. Unknown commands stay COMMAND_UNKNOWN.
        let spec = self.state.registry.describe(command).ok_or_else(|| {
            WireError::new(
                proto::COMMAND_UNKNOWN,
                "COMMAND_UNKNOWN",
                format!("unknown command: {command}"),
            )
        })?;
        if spec.effect > role.max_effect() {
            return Err(WireError::capability_denied(format!(
                "role '{}' is capped at effect '{}'; '{command}' has effect '{}'",
                role.as_str(),
                role.max_effect(),
                spec.effect,
            )));
        }

        let result = {
            let mut vault = self.state.vault.lock().unwrap();
            self.state
                .registry
                .invoke(&mut vault, command, &input)
                .map_err(|e| proto::map_vault_error(&e))?
        };

        // Post-commit events for the fixed surface's mutating commands.
        match command {
            "document.save" => {
                self.state.events.publish(
                    "revision.committed",
                    json!({
                        "doc": result["doc"], "rev": result["rev"],
                        "object": result["object"], "path": result["path"],
                        "origin": input.get("origin").and_then(Value::as_str).unwrap_or("editor"),
                    }),
                );
            }
            "vault.scan" => {
                if let Some(converted) = result["converted"].as_array() {
                    for c in converted {
                        self.state.events.publish("doc.external_change", c.clone());
                        let mut payload = c.clone();
                        payload["origin"] = json!("external");
                        self.state.events.publish("revision.committed", payload);
                    }
                }
            }
            _ => {}
        }

        Ok(result)
    }

    fn subscribe(&mut self, params: &Value) -> Result<Value, WireError> {
        self.authed()?;
        let topics = params
            .get("topics")
            .and_then(Value::as_array)
            .ok_or_else(|| WireError::validation_failed("'topics' (array) is required"))?;
        let mut granted = Vec::new();
        for t in topics {
            let name = t
                .as_str()
                .ok_or_else(|| WireError::validation_failed("topics must be strings"))?;
            if !TOPICS.contains(&name) {
                return Err(WireError::validation_failed(format!(
                    "unknown topic '{name}'"
                )));
            }
            self.topics.insert(name.to_owned());
            granted.push(json!({"topic": name, "seq": self.state.events.seq(name)}));
        }
        Ok(json!({"subscribed": granted}))
    }
}

pub fn event_notification(event: &Event) -> String {
    proto::notification(
        "event",
        json!({"topic": event.topic, "seq": event.seq, "payload": event.payload}),
    )
}
