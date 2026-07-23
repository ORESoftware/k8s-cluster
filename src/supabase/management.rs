//! Supabase Management API — used ONLY by the offline `discover` subcommand.
//!
//! This is the one place the account-level Personal Access Token (`sbp_…`) is used. It
//! can read and *delete* projects, so it must never touch the request path — it
//! only enumerates the account's orgs/projects and prints a ready-to-paste
//! `AUTH_SUPABASE_PROJECTS` value for the server config.
//!
//! Run: `SUPABASE_ACCESS_TOKEN=sbp_… shared-auth-server discover`

use anyhow::Context;
use serde::Deserialize;

const MANAGEMENT_BASE: &str = "https://api.supabase.com/v1";

#[derive(Debug, Deserialize)]
struct Organization {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Project {
    #[serde(rename = "id")]
    project_ref: String,
    name: String,
    organization_id: String,
    region: String,
}

/// Enumerate orgs + projects for the token's account and print a config skeleton.
pub async fn discover() -> anyhow::Result<()> {
    let token = std::env::var("SUPABASE_ACCESS_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .context("SUPABASE_ACCESS_TOKEN (sbp_… personal access token) is required for discover")?;

    let http = reqwest::Client::builder()
        .user_agent("shared-auth-server/0.1 (discover)")
        .build()?;

    let orgs: Vec<Organization> = get(&http, &token, "/organizations")
        .await
        .context("listing organizations")?;
    let projects: Vec<Project> = get(&http, &token, "/projects")
        .await
        .context("listing projects")?;

    eprintln!(
        "# Discovered {} orgs, {} projects",
        orgs.len(),
        projects.len()
    );
    for org in &orgs {
        eprintln!("#   org {} ({})", org.name, org.id);
    }

    // Emit a JSON array suitable for AUTH_SUPABASE_PROJECTS. `name` is slugified
    // from the org name so the mirror column is human-readable and stable.
    let entries: Vec<serde_json::Value> = projects
        .iter()
        .map(|p| {
            let org_name = orgs
                .iter()
                .find(|o| o.id == p.organization_id)
                .map(|o| o.name.as_str())
                .unwrap_or(&p.name);
            let name = slugify(org_name);
            let env_prefix = name.to_ascii_uppercase().replace('-', "_");
            serde_json::json!({
                "name": name,
                "project_ref": p.project_ref,
                "audience": "authenticated",
                "publishable_key_env": format!("AUTH_SUPABASE_{env_prefix}_PUBLISHABLE_KEY"),
                "secret_key_env": format!("AUTH_SUPABASE_{env_prefix}_SECRET_KEY"),
                "_region": p.region,
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    eprintln!(
        "\n# Paste the above as AUTH_SUPABASE_PROJECTS (drop the _region hint fields).\n\
         # issuer/jwks_url are derived from project_ref automatically. Store each
         # referenced API key as a separate Fiducia-managed environment secret."
    );
    Ok(())
}

async fn get<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    token: &str,
    path: &str,
) -> anyhow::Result<T> {
    let resp = http
        .get(format!("{MANAGEMENT_BASE}{path}"))
        .bearer_auth(token)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("management API {path} returned {status}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("decoding {path} response"))
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .replace("--", "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes_org_names() {
        assert_eq!(slugify("Fiducia Cloud"), "fiducia-cloud");
        assert_eq!(slugify("3fa-app"), "3fa-app");
        assert_eq!(slugify("Athlet-O Store"), "athlet-o-store");
        assert_eq!(slugify("--trim--"), "trim");
    }
}
