//! The command registry (ARCHITECTURE.md §7): every keyboard action, menu
//! item, voice action, extension, and agent invocation addresses this one
//! surface. The registry *is* the API — `commands.invoke` invokes exactly
//! these, and there is deliberately no second, ad-hoc surface.
//!
//! Effect classes order the blast radius (`read < propose < commit <
//! system`); the RPC boundary caps each connection role at a maximum effect
//! (§6). Inputs are validated against each command's published JSON Schema —
//! the same document `commands.describe` serves, so enforcement and
//! documentation cannot drift apart.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::VaultError;
use crate::ids::RevisionId;
use crate::journal::RevisionOrigin;
use crate::vault::Vault;
use crate::writer::{DocRef, SaveRequest};

/// The four effect classes. Order matters: a role capped at `Propose` may
/// invoke `Read` and `Propose` commands, nothing above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Read,
    Propose,
    Commit,
    System,
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Effect::Read => "read",
            Effect::Propose => "propose",
            Effect::Commit => "commit",
            Effect::System => "system",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkPolicy {
    Never,
    ProviderOnly,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceClass {
    Interactive,
    Background,
    Maintenance,
}

/// One command's registration record (§7's `Command` interface).
#[derive(Debug, Clone, Serialize)]
pub struct CommandSpec {
    pub id: String,
    pub summary: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capabilities: Vec<String>,
    pub effect: Effect,
    pub network: NetworkPolicy,
    pub context_policy: String,
    pub undo_policy: String,
    pub resource_class: ResourceClass,
    pub default_keys: Vec<String>,
}

pub type CommandHandler = Box<dyn Fn(&mut Vault, Value) -> Result<Value, VaultError> + Send + Sync>;

struct Command {
    spec: CommandSpec,
    validator: jsonschema::Validator,
    handler: CommandHandler,
}

/// The one command surface. Registration validates the schemas themselves;
/// invocation validates every input against them.
#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry pre-loaded with the Phase-1 built-ins.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        register_builtins(&mut r).expect("built-in command registration is infallible");
        r
    }

    pub fn register(
        &mut self,
        spec: CommandSpec,
        handler: CommandHandler,
    ) -> Result<(), VaultError> {
        if self.commands.contains_key(&spec.id) {
            return Err(VaultError::ValidationFailed {
                reason: format!("command '{}' already registered", spec.id),
            });
        }
        let validator = jsonschema::validator_for(&spec.input_schema).map_err(|e| {
            VaultError::ValidationFailed {
                reason: format!("invalid input schema for '{}': {e}", spec.id),
            }
        })?;
        // Output schemas must at least be valid schemas, even though Phase 1
        // does not validate outputs on the hot path.
        jsonschema::validator_for(&spec.output_schema).map_err(|e| {
            VaultError::ValidationFailed {
                reason: format!("invalid output schema for '{}': {e}", spec.id),
            }
        })?;
        self.commands.insert(
            spec.id.clone(),
            Command {
                spec,
                validator,
                handler,
            },
        );
        Ok(())
    }

    /// Registry enumeration — drives the command palette (`commands.list`).
    pub fn list(&self) -> Vec<&CommandSpec> {
        self.commands.values().map(|c| &c.spec).collect()
    }

    /// Full schema for one command (`commands.describe`).
    pub fn describe(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.get(id).map(|c| &c.spec)
    }

    /// Validate `input` and run the command against the vault.
    pub fn invoke(&self, vault: &mut Vault, id: &str, input: &Value) -> Result<Value, VaultError> {
        let cmd = self
            .commands
            .get(id)
            .ok_or_else(|| VaultError::CommandUnknown(id.to_owned()))?;
        if let Err(e) = cmd.validator.validate(input) {
            return Err(VaultError::ValidationFailed {
                reason: format!("{id}: {e}"),
            });
        }
        (cmd.handler)(vault, input.clone())
    }
}

impl fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandRegistry")
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn str_field(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn register_builtins(r: &mut CommandRegistry) -> Result<(), VaultError> {
    let object_schema = |props: Value, required: &[&str]| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": required,
            "properties": props,
        })
    };

    // document.save — the one commit path clients get. `origin` is capped at
    // editor|import: `external` belongs to the scanner and `proposal-accept`
    // to the Phase-3 accept flow; neither is client-claimable.
    r.register(
        CommandSpec {
            id: "document.save".into(),
            summary: "Save content to a vault path, based on a revision".into(),
            input_schema: object_schema(
                json!({
                    "path": {"type": "string", "minLength": 1},
                    "content": {"type": "string"},
                    "base": {"type": ["string", "null"]},
                    "origin": {"type": "string", "enum": ["editor", "import"]},
                }),
                &["path", "content"],
            ),
            output_schema: object_schema(
                json!({
                    "doc": {"type": "string"},
                    "rev": {"type": "string"},
                    "object": {"type": "string"},
                    "path": {"type": "string"},
                }),
                &["doc", "rev", "object", "path"],
            ),
            capabilities: vec!["vault.write".into()],
            effect: Effect::Commit,
            network: NetworkPolicy::Never,
            context_policy: "document".into(),
            undo_policy: "new-revision".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec!["Mod-s".into()],
        },
        Box::new(|vault, input| {
            let base = match input.get("base") {
                Some(Value::String(s)) => Some(RevisionId::from_string(s.clone())),
                _ => None,
            };
            let origin = match input.get("origin").and_then(Value::as_str) {
                Some("import") => RevisionOrigin::Import,
                _ => RevisionOrigin::Editor,
            };
            let out = vault.writer()?.save(SaveRequest {
                doc: DocRef::Path(str_field(&input, "path")),
                base,
                content: str_field(&input, "content").into_bytes(),
                origin,
                lease: None,
            })?;
            Ok(json!({
                "doc": out.doc, "rev": out.rev, "object": out.object, "path": out.path,
            }))
        }),
    )?;

    r.register(
        CommandSpec {
            id: "document.read".into(),
            summary: "Read a document's canonical content at its head".into(),
            input_schema: object_schema(
                json!({"path": {"type": "string", "minLength": 1}}),
                &["path"],
            ),
            output_schema: object_schema(
                json!({
                    "doc": {"type": "string"},
                    "rev": {"type": "string"},
                    "object": {"type": "string"},
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                }),
                &["doc", "rev", "object", "path", "content"],
            ),
            capabilities: vec!["vault.read".into()],
            effect: Effect::Read,
            network: NetworkPolicy::Never,
            context_policy: "document".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec![],
        },
        Box::new(|vault, input| {
            let path = str_field(&input, "path");
            let doc = vault
                .index()
                .doc_by_path(&path)
                .ok_or_else(|| VaultError::DocNotFound(path.clone()))?
                .clone();
            let head = vault
                .index()
                .head(&doc)
                .ok_or_else(|| VaultError::DocNotFound(path.clone()))?
                .clone();
            let bytes = vault.objects().read(&head.object)?;
            Ok(json!({
                "doc": doc, "rev": head.rev, "object": head.object, "path": head.path,
                "content": String::from_utf8_lossy(&bytes),
            }))
        }),
    )?;

    r.register(
        CommandSpec {
            id: "document.list".into(),
            summary: "List every document head".into(),
            input_schema: object_schema(json!({}), &[]),
            output_schema: object_schema(json!({"documents": {"type": "array"}}), &["documents"]),
            capabilities: vec!["vault.read".into()],
            effect: Effect::Read,
            network: NetworkPolicy::Never,
            context_policy: "vault".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec![],
        },
        Box::new(|vault, _| {
            let mut docs: Vec<_> = vault
                .index()
                .iter_heads()
                .map(|(doc, head)| {
                    json!({"doc": doc, "path": head.path, "rev": head.rev, "object": head.object})
                })
                .collect();
            docs.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
            Ok(json!({"documents": docs}))
        }),
    )?;

    r.register(
        CommandSpec {
            id: "document.history".into(),
            summary: "Full linear revision history of one document".into(),
            input_schema: object_schema(
                json!({"path": {"type": "string", "minLength": 1}}),
                &["path"],
            ),
            output_schema: object_schema(json!({"revisions": {"type": "array"}}), &["revisions"]),
            capabilities: vec!["vault.read".into()],
            effect: Effect::Read,
            network: NetworkPolicy::Never,
            context_policy: "document".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec![],
        },
        Box::new(|vault, input| {
            let path = str_field(&input, "path");
            let doc = vault
                .index()
                .doc_by_path(&path)
                .ok_or_else(|| VaultError::DocNotFound(path.clone()))?
                .clone();
            let revisions: Vec<_> = vault
                .history(&doc)?
                .into_iter()
                .map(|r| {
                    json!({
                        "rev": r.rev, "parent": r.parent, "object": r.object,
                        "origin": r.origin, "ts": r.ts, "path": r.path,
                    })
                })
                .collect();
            Ok(json!({"doc": doc, "revisions": revisions}))
        }),
    )?;

    r.register(
        CommandSpec {
            id: "search.query".into(),
            summary: "Full-text search over document bodies (FTS5 syntax)".into(),
            input_schema: object_schema(
                json!({
                    "query": {"type": "string", "minLength": 1},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500},
                }),
                &["query"],
            ),
            output_schema: object_schema(json!({"hits": {"type": "array"}}), &["hits"]),
            capabilities: vec!["vault.read".into()],
            effect: Effect::Read,
            network: NetworkPolicy::Never,
            context_policy: "vault".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec!["Mod-k".into()],
        },
        Box::new(|vault, input| {
            let query = str_field(&input, "query");
            let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(20) as u32;
            let hits: Vec<_> = vault
                .search(&query, limit)?
                .into_iter()
                .map(|h| json!({"doc": h.doc, "path": h.path, "snippet": h.snippet}))
                .collect();
            Ok(json!({"hits": hits}))
        }),
    )?;

    // vault.scan commits external revisions, so its effect class is commit —
    // an agent-role connection cannot trigger it.
    r.register(
        CommandSpec {
            id: "vault.scan".into(),
            summary: "Convert out-of-band edits under vault/ into external revisions".into(),
            input_schema: object_schema(json!({}), &[]),
            output_schema: object_schema(
                json!({"converted": {"type": "array"}, "missing": {"type": "array"}}),
                &["converted", "missing"],
            ),
            capabilities: vec!["vault.write".into()],
            effect: Effect::Commit,
            network: NetworkPolicy::Never,
            context_policy: "vault".into(),
            undo_policy: "new-revision".into(),
            resource_class: ResourceClass::Background,
            default_keys: vec![],
        },
        Box::new(|vault, _| {
            let scan = vault.scan_external()?;
            let converted: Vec<_> = scan
                .converted
                .iter()
                .map(|o| json!({"doc": o.doc, "path": o.path, "rev": o.rev, "object": o.object}))
                .collect();
            Ok(json!({"converted": converted, "missing": scan.missing}))
        }),
    )?;

    r.register(
        CommandSpec {
            id: "vault.status".into(),
            summary: "Vault identity, format, and index availability".into(),
            input_schema: object_schema(json!({}), &[]),
            output_schema: object_schema(
                json!({
                    "vault_id": {"type": "string"},
                    "vault_format": {"type": "integer"},
                    "documents": {"type": "integer"},
                    "index_available": {"type": "boolean"},
                    "warnings": {"type": "array"},
                }),
                &[
                    "vault_id",
                    "vault_format",
                    "documents",
                    "index_available",
                    "warnings",
                ],
            ),
            capabilities: vec!["vault.read".into()],
            effect: Effect::Read,
            network: NetworkPolicy::Never,
            context_policy: "vault".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Interactive,
            default_keys: vec![],
        },
        Box::new(|vault, _| {
            Ok(json!({
                "vault_id": vault.format().vault_id,
                "vault_format": vault.format().vault_format,
                "documents": vault.index().len(),
                "index_available": vault.derived().is_some(),
                "warnings": vault.warnings(),
            }))
        }),
    )?;

    // The §7 exemplar of a system-effect command: exists on every profile
    // with an identical schema, and resolves to dry-run below appliance
    // profiles. Every Phase-1 profile is a dev profile, so dry_run is
    // always true here; the appliance flips enforcement, not the schema.
    r.register(
        CommandSpec {
            id: "system.health.inspect".into(),
            summary: "Inspect vault health (dry-run on dev profiles)".into(),
            input_schema: object_schema(json!({}), &[]),
            output_schema: object_schema(
                json!({"dry_run": {"type": "boolean"}, "checks": {"type": "array"}}),
                &["dry_run", "checks"],
            ),
            capabilities: vec!["system.inspect".into()],
            effect: Effect::System,
            network: NetworkPolicy::Never,
            context_policy: "system".into(),
            undo_policy: "none".into(),
            resource_class: ResourceClass::Maintenance,
            default_keys: vec![],
        },
        Box::new(|vault, _| {
            let journal_segments = std::fs::read_dir(vault.root().join("journal"))
                .map(|d| {
                    d.filter_map(Result::ok)
                        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                        .count()
                })
                .unwrap_or(0);
            let checks = json!([
                {"check": "journal_segments", "value": journal_segments, "ok": journal_segments > 0},
                {"check": "objects_dir", "ok": vault.root().join("objects/sha256").is_dir()},
                {"check": "derived_index", "ok": vault.derived().is_some()},
                {"check": "documents", "value": vault.index().len(), "ok": true},
                {"check": "reconcile_warnings", "value": vault.warnings().len(),
                 "ok": vault.warnings().is_empty()},
            ]);
            Ok(json!({"dry_run": true, "checks": checks}))
        }),
    )?;

    Ok(())
}
