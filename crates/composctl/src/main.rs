use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use compos_core::{DocRef, HostProfile, RevisionId, RevisionOrigin, SaveRequest, Vault};

#[derive(Parser)]
#[command(name = "composctl", version, about = "CompOS vault client")]
struct Cli {
    /// Vault root. Defaults to $COMPOS_VAULT, then the host profile location.
    #[arg(long, global = true)]
    vault: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new vault.
    Init,
    /// Save content to a vault path (reads stdin unless --file is given).
    /// Bases the save on the current head unless --base pins a revision.
    Save {
        path: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        base: Option<String>,
    },
    /// List document heads, or one document's full revision history.
    Log { path: Option<String> },
    /// Print a document's canonical content from its head object.
    Cat { path: String },
    /// Full-text search over document bodies (FTS5 syntax).
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Convert out-of-band edits under vault/ into external revisions.
    Scan,
    /// Print version information.
    Version,
    /// Torture-harness child process (internal).
    #[command(name = "_torture-child", hide = true)]
    TortureChild {
        #[arg(long, default_value_t = 8)]
        docs: u32,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = vault_root(cli.vault)?;

    match cli.command {
        Command::Init => {
            let vault = Vault::init(&root)?;
            print_warnings(&vault);
            println!("initialized vault at {}", root.display());
        }
        Command::Save { path, file, base } => {
            let content = match file {
                Some(f) => std::fs::read(&f).with_context(|| format!("reading {}", f.display()))?,
                None => {
                    let mut buf = Vec::new();
                    std::io::stdin().read_to_end(&mut buf)?;
                    buf
                }
            };
            let mut vault = Vault::open_write(&root)?;
            print_warnings(&vault);
            let base = match base {
                Some(rev) => Some(RevisionId::from_string(rev)),
                None => vault
                    .index()
                    .doc_by_path(&path)
                    .and_then(|d| vault.index().head(d))
                    .map(|h| h.rev.clone()),
            };
            let out = vault.writer()?.save(SaveRequest {
                doc: DocRef::Path(path),
                base,
                content,
                origin: RevisionOrigin::Editor,
                lease: None,
            })?;
            println!("{} {} {}", out.path, out.rev, out.object);
        }
        Command::Log { path } => {
            let vault = Vault::open_read(&root)?;
            match path {
                None => {
                    let mut heads: Vec<_> = vault.index().iter_heads().collect();
                    heads.sort_by(|a, b| a.1.path.cmp(&b.1.path));
                    for (doc, head) in heads {
                        println!("{}\t{}\t{}\t{}", head.path, doc, head.rev, head.object);
                    }
                }
                Some(p) => {
                    let doc = vault
                        .index()
                        .doc_by_path(&p)
                        .with_context(|| format!("no document at '{p}'"))?
                        .clone();
                    for rec in vault.history(&doc)? {
                        println!(
                            "{}\t{}\t{:?}\t{}\t{}",
                            rec.rev,
                            rec.parent
                                .as_ref()
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "-".to_owned()),
                            rec.origin,
                            rec.ts,
                            rec.object
                        );
                    }
                }
            }
        }
        Command::Cat { path } => {
            let vault = Vault::open_read(&root)?;
            let doc = vault
                .index()
                .doc_by_path(&path)
                .with_context(|| format!("no document at '{path}'"))?;
            let head = vault.index().head(doc).context("missing head")?;
            let bytes = vault.objects().read(&head.object)?;
            std::io::stdout().write_all(&bytes)?;
        }
        Command::Search { query, limit } => {
            // A read-only open serves a fresh index; if none exists yet,
            // fall back to a write open, which builds it from the journal.
            let read = Vault::open_read(&root)?;
            let hits = if read.derived().is_some() {
                read.search(&query, limit)?
            } else {
                drop(read);
                let vault = Vault::open_write(&root)?;
                print_warnings(&vault);
                vault.search(&query, limit)?
            };
            for hit in hits {
                println!("{}\t{}\t{}", hit.path, hit.doc, hit.snippet);
            }
        }
        Command::Scan => {
            let mut vault = Vault::open_write(&root)?;
            print_warnings(&vault);
            let scan = vault.scan_external()?;
            for out in &scan.converted {
                println!("external {} {} {}", out.path, out.rev, out.object);
            }
            for path in &scan.missing {
                eprintln!("warning: '{path}' has revisions but no visible file");
            }
            if scan.converted.is_empty() {
                println!("no external changes");
            }
        }
        Command::Version => {
            println!(
                "composctl {} (vault format {})",
                compos_core::VERSION,
                compos_core::VAULT_FORMAT
            );
        }
        Command::TortureChild { docs } => torture_child(&root, docs)?,
    }
    Ok(())
}

fn vault_root(cli_vault: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(v) = cli_vault {
        return Ok(v);
    }
    if let Some(v) = std::env::var_os("COMPOS_VAULT")
        && !v.is_empty()
    {
        return Ok(PathBuf::from(v));
    }
    HostProfile::resolve()
        .default_vault_root()
        .context("cannot determine a default vault root; pass --vault")
}

fn print_warnings(vault: &Vault) {
    for w in vault.warnings() {
        eprintln!("warning: {w}");
    }
}

/// Loop forever making randomized saves, printing an ACK line only after
/// each save's journal fsync. The torture test kills this process at random
/// points and verifies that no acknowledged write is ever lost.
fn torture_child(root: &std::path::Path, docs: u32) -> anyhow::Result<()> {
    let mut vault = if root.join("compos.json").exists() {
        Vault::open_write(root)?
    } else {
        Vault::init(root)?
    };

    let mut rng = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos() as u64
        ^ (std::process::id() as u64) << 32
        | 1;
    let mut stdout = std::io::stdout();
    let mut iter: u64 = 0;

    loop {
        iter += 1;
        let di = xorshift(&mut rng) % docs as u64;
        let path = format!("notes/doc-{di}.md");
        let base = vault
            .index()
            .doc_by_path(&path)
            .and_then(|d| vault.index().head(d))
            .map(|h| h.rev.clone());

        let extra = (xorshift(&mut rng) % (64 * 1024)) as usize;
        let mut content = format!("# doc {di}\niteration {iter}\n").into_bytes();
        while content.len() < extra {
            content.extend_from_slice(&xorshift(&mut rng).to_le_bytes());
        }

        let out = vault.writer()?.save(SaveRequest {
            doc: DocRef::Path(path),
            base,
            content,
            origin: RevisionOrigin::Editor,
            lease: None,
        })?;
        // The save has been fsynced into the journal; only now acknowledge.
        writeln!(stdout, "ACK {} {} {}", out.path, out.rev, out.object)?;
        stdout.flush()?;
    }
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}
