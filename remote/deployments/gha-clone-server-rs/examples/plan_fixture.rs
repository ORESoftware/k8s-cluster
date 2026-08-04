use std::{env, fs, process};

use gha_clone_server::{build_plan, PlanRequest, PlannerLimits};

fn usage() -> ! {
    eprintln!(
        "usage: plan_fixture OWNER/REPO 40_HEX_SHA .github/workflows/FILE.yml WORKFLOW_FILE"
    );
    process::exit(2);
}

fn main() {
    let mut arguments = env::args().skip(1);
    let repository = arguments.next().unwrap_or_else(|| usage());
    let revision = arguments.next().unwrap_or_else(|| usage());
    let workflow_path = arguments.next().unwrap_or_else(|| usage());
    let source_path = arguments.next().unwrap_or_else(|| usage());
    if arguments.next().is_some() {
        usage();
    }

    let workflow_yaml = fs::read_to_string(&source_path).unwrap_or_else(|error| {
        eprintln!("failed to read {source_path}: {error}");
        process::exit(2);
    });
    let request = PlanRequest {
        repository,
        revision,
        workflow_path,
        workflow_yaml,
    };
    let plan = build_plan(&request, &PlannerLimits::default()).unwrap_or_else(|errors| {
        for error in errors {
            eprintln!("plan rejected: {error}");
        }
        process::exit(1);
    });
    serde_json::to_writer_pretty(std::io::stdout(), &plan).expect("serialize workflow plan");
    println!();
}
