mod inspect;
mod mcp;
mod model;

use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("mcp") => mcp::serve(),
        Some("inspect") | Some("doctor") => run_inspect(args.collect()),
        Some("--version") | Some("-V") => {
            println!("zed-air {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => bail!("unknown command `{other}`; run `zed-air help`"),
    }
}

fn print_help() {
    println!(
        "zed-air {version}\n\nUSAGE:\n  zed-air mcp\n  zed-air inspect [--root PATH] [--json]\n  zed-air doctor [--root PATH] [--json]\n\nThe MCP server is read-only. Recommended commands are returned to the Air agent,\nbut project-changing commands are never executed by zed-air.",
        version = env!("CARGO_PKG_VERSION")
    );
}

fn run_inspect(args: Vec<String>) -> Result<()> {
    let mut root = env::current_dir().context("resolve current directory")?;
    let mut json_output = false;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => {
                root = PathBuf::from(
                    iter.next()
                        .ok_or_else(|| anyhow!("--root requires a path"))?,
                );
            }
            "--json" => json_output = true,
            other => bail!("unknown inspect argument `{other}`"),
        }
    }

    let report = inspect::project(&root)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", inspect::render(&report));
    }
    Ok(())
}
