use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
    {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            collect_rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

#[test]
fn main_contains_only_process_bootstrap_work() {
    let main = read("src/main.rs");
    assert!(
        main.lines().count() <= 60,
        "src/main.rs must remain process bootstrap code; found {} lines",
        main.lines().count()
    );

    for module in [
        "app",
        "data",
        "database",
        "routes",
        "shutdown",
        "telemetry",
        "views",
    ] {
        assert!(
            main.contains(&format!("mod {module};")),
            "src/main.rs must declare the {module} module"
        );
    }

    for implementation_detail in [
        "Router::new",
        "sea_orm::",
        "opentelemetry::",
        ".route(",
        "struct ",
        "impl ",
        "\"/healthz\"",
        "\"/readyz\"",
    ] {
        assert!(
            !main.contains(implementation_detail),
            "src/main.rs owns implementation detail {implementation_detail:?}"
        );
    }
}

#[test]
fn modules_own_their_expected_responsibilities() {
    let expectations = [
        ("src/app.rs", "pub(crate) struct AppState"),
        ("src/data.rs", "pub(crate) struct DashboardStats"),
        ("src/database.rs", "pub(crate) struct DatabaseState"),
        ("src/routes.rs", "pub(crate) fn router"),
        ("src/shutdown.rs", "pub(crate) async fn signal"),
        ("src/telemetry.rs", "pub(crate) fn init"),
        ("src/views.rs", "pub(crate) fn render_page"),
    ];

    for (module, public_boundary) in expectations {
        let source = read(module);
        assert!(
            source.contains(public_boundary),
            "{module} must retain its boundary {public_boundary:?}"
        );
    }

    let routes = read("src/routes.rs");
    for dependency in ["app::", "data::", "database::", "telemetry", "views"] {
        assert!(
            routes.contains(dependency),
            "routes must compose the {dependency} module boundary"
        );
    }
    assert!(routes.contains(".route(\"/healthz\""));
    assert!(routes.contains(".route(\"/readyz\""));
}

#[test]
fn leaf_modules_do_not_depend_back_on_routes() {
    for module in [
        "src/app.rs",
        "src/data.rs",
        "src/database.rs",
        "src/shutdown.rs",
        "src/telemetry.rs",
        "src/views.rs",
    ] {
        let source = read(module);
        assert!(
            !source.contains("crate::routes") && !source.contains("super::routes"),
            "{module} must not depend back on the route-composition module"
        );
    }
}

#[test]
fn persistence_boundary_is_seaorm_without_direct_sqlx() {
    let cargo_toml = read("Cargo.toml");
    assert!(
        cargo_toml
            .lines()
            .any(|line| line.trim_start().starts_with("sea-orm =")),
        "Cargo.toml must keep SeaORM as the service persistence boundary"
    );
    assert!(
        !cargo_toml.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("sqlx =") || line.starts_with("sqlx=")
        }),
        "Cargo.toml must not declare SQLx directly; SeaORM owns that implementation detail"
    );

    let mut sources = Vec::new();
    collect_rust_sources(&repo_root().join("src"), &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert!(
            !source.contains("use sqlx") && !source.contains("sqlx::"),
            "{} imports SQLx directly",
            path.display()
        );
    }

    let database = read("src/database.rs");
    assert!(database.contains("use sea_orm::"));
    assert!(database.contains("AKRION_DATABASE_URL"));
}
