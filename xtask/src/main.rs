use ark::config::ArkConfig;
use chrono::Utc;
use clap::{Parser, Subcommand};
use schemars::schema_for;
use std::{env, fs, path::PathBuf};

#[derive(Parser)]
#[command(about = "dev helper tasks for ark (xtask)")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new SQL migration file
    NewMigration {
        /// Short description for the migration (quoted if contains spaces)
        name: String,
        /// Backend directory (sqlite, postgres). Defaults to sqlite.
        #[arg(long, default_value = "sqlite")]
        backend: String,
    },
    /// Generate JSON schema for ArkConfig
    GenerateSchema,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::NewMigration { name, backend } => create_migration(&name, &backend)?,
        Command::GenerateSchema => generate_schema()?,
    }
    Ok(())
}

fn create_migration(name: &str, backend: &str) -> Result<(), Box<dyn std::error::Error>> {
    let repo = env::current_dir()?;
    let dir = repo.join("migrations").join(backend);
    fs::create_dir_all(&dir)?;

    let mut max: u32 = 0;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if let Some(n) = entry.file_name().to_str() {
            if let Some(prefix) = n.split("__").next() {
                if prefix.starts_with('V') {
                    if let Ok(v) = prefix[1..].parse::<u32>() {
                        if v > max {
                            max = v;
                        }
                    }
                }
            }
        }
    }

    let next = max + 1;
    let safe: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let fname = format!("V{:03}__{}.sql", next, safe);
    let path = dir.join(&fname);
    let now = Utc::now().to_rfc3339();
    let body = format!(
        "-- Migration: {}\n-- Generated: {}\n\nBEGIN;\n-- up\n\nCOMMIT;\n",
        name, now
    );
    fs::write(&path, body)?;
    println!("Created {}", path.display());
    Ok(())
}

fn generate_schema() -> Result<(), Box<dyn std::error::Error>> {
    let schema = schema_for!(ArkConfig);
    let json = serde_json::to_string_pretty(&schema)?;
    let repo = env::current_dir()?.parent().unwrap().to_path_buf(); // Go up to ark root
    let path = repo.join("www").join("ark.config.schema.json");
    fs::write(&path, json)?;
    println!("Schema generated and written to {}", path.display());
    Ok(())
}
