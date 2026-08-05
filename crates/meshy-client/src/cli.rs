use std::{
    env,
    error::Error,
    fs,
    io::{self, Read},
    process,
    time::Duration,
};

use serde::{de::DeserializeOwned, Serialize};

use crate::{
    capability_document, ImageTo3dRequest, ListTasksQuery, MeshyClient, MeshyError,
    MultiImageTo3dRequest, TaskKind, WaitOptions,
};

pub async fn run_from_env() -> Result<(), CliError> {
    run(env::args().skip(1).collect()).await
}

pub async fn run(args: Vec<String>) -> Result<(), CliError> {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Err(CliError::Usage("missing command".to_string()));
    };

    match command {
        "capabilities" => {
            print_json(&capability_document())?;
            return Ok(());
        }
        "help" | "--help" | "-h" => {
            print_usage();
            return Ok(());
        }
        _ => {}
    }

    let client = MeshyClient::from_env()?;
    match command {
        "create-image" => {
            let request: ImageTo3dRequest = read_request(argument(&args, 1, "request file")?)?;
            let created = client.create_image_task(&request).await?;
            print_json(&created.daedalus_receipt(TaskKind::ImageTo3d))?;
        }
        "create-multi-image" => {
            let request: MultiImageTo3dRequest =
                read_request(argument(&args, 1, "request file")?)?;
            let created = client.create_multi_image_task(&request).await?;
            print_json(&created.daedalus_receipt(TaskKind::MultiImageTo3d))?;
        }
        "get-image" => {
            print_candidate(
                &client,
                TaskKind::ImageTo3d,
                argument(&args, 1, "task id")?,
            )
            .await?;
        }
        "get-multi-image" => {
            print_candidate(
                &client,
                TaskKind::MultiImageTo3d,
                argument(&args, 1, "task id")?,
            )
            .await?;
        }
        "wait-image" => {
            wait_and_print(
                &client,
                TaskKind::ImageTo3d,
                argument(&args, 1, "task id")?,
                optional_u64(&args, 2, 1_800, "timeout seconds")?,
            )
            .await?;
        }
        "wait-multi-image" => {
            wait_and_print(
                &client,
                TaskKind::MultiImageTo3d,
                argument(&args, 1, "task id")?,
                optional_u64(&args, 2, 1_800, "timeout seconds")?,
            )
            .await?;
        }
        "list-image" => {
            let query = list_query(&args)?;
            print_json(&client.list_tasks(TaskKind::ImageTo3d, query).await?)?;
        }
        "list-multi-image" => {
            let query = list_query(&args)?;
            print_json(&client.list_tasks(TaskKind::MultiImageTo3d, query).await?)?;
        }
        "delete-image" => {
            let task_id = argument(&args, 1, "task id")?;
            client.delete_task(TaskKind::ImageTo3d, task_id).await?;
            print_json(&serde_json::json!({"ok": true, "deletedTaskId": task_id}))?;
        }
        "delete-multi-image" => {
            let task_id = argument(&args, 1, "task id")?;
            client
                .delete_task(TaskKind::MultiImageTo3d, task_id)
                .await?;
            print_json(&serde_json::json!({"ok": true, "deletedTaskId": task_id}))?;
        }
        unknown => {
            print_usage();
            return Err(CliError::Usage(format!("unknown command: {unknown}")));
        }
    }
    Ok(())
}

async fn print_candidate(
    client: &MeshyClient,
    kind: TaskKind,
    task_id: &str,
) -> Result<(), CliError> {
    let task = client.get_task(kind, task_id).await?;
    print_json(&task.daedalus_candidate(kind))
}

async fn wait_and_print(
    client: &MeshyClient,
    kind: TaskKind,
    task_id: &str,
    timeout_seconds: u64,
) -> Result<(), CliError> {
    let task = client
        .wait_for_task(
            kind,
            task_id,
            WaitOptions {
                timeout: Duration::from_secs(timeout_seconds),
                ..WaitOptions::default()
            },
        )
        .await?;
    print_json(&task.daedalus_candidate(kind))
}

fn argument<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str, CliError> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| CliError::Usage(format!("missing {name}")))
}

fn optional_u64(
    args: &[String],
    index: usize,
    fallback: u64,
    name: &str,
) -> Result<u64, CliError> {
    args.get(index)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::Usage(format!("invalid {name}: {value}")))
        })
        .transpose()
        .map(|value| value.unwrap_or(fallback))
}

fn list_query(args: &[String]) -> Result<ListTasksQuery, CliError> {
    let page_num = optional_u64(args, 1, 1, "page number")?;
    let page_size = optional_u64(args, 2, 10, "page size")?;
    Ok(ListTasksQuery {
        page_num: u32::try_from(page_num)
            .map_err(|_| CliError::Usage("page number exceeds u32".to_string()))?,
        page_size: u16::try_from(page_size)
            .map_err(|_| CliError::Usage("page size exceeds u16".to_string()))?,
        ..ListTasksQuery::default()
    })
}

fn read_request<T: DeserializeOwned>(path: &str) -> Result<T, CliError> {
    let content = if path == "-" {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        content
    } else {
        fs::read_to_string(path)?
    };
    serde_json::from_str(&content).map_err(CliError::Json)
}

fn print_json<T: Serialize>(value: &T) -> Result<(), CliError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_usage() {
    eprintln!(
        "dd-meshy-adapter\n\
         \n\
         Usage:\n\
           dd-meshy-adapter capabilities\n\
           dd-meshy-adapter create-image <request.json|->\n\
           dd-meshy-adapter create-multi-image <request.json|->\n\
           dd-meshy-adapter get-image <task-id>\n\
           dd-meshy-adapter get-multi-image <task-id>\n\
           dd-meshy-adapter wait-image <task-id> [timeout-seconds]\n\
           dd-meshy-adapter wait-multi-image <task-id> [timeout-seconds]\n\
           dd-meshy-adapter list-image [page-number] [page-size]\n\
           dd-meshy-adapter list-multi-image [page-number] [page-size]\n\
           dd-meshy-adapter delete-image <task-id>\n\
           dd-meshy-adapter delete-multi-image <task-id>\n\
         \n\
         Environment:\n\
           MESHY_API_KEY       required bearer credential\n\
           MESHY_API_BASE_URL  optional; defaults to https://api.meshy.ai"
    );
}

#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Io(io::Error),
    Json(serde_json::Error),
    Meshy(MeshyError),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::Meshy(error) => error.fmt(f),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Meshy(error) => Some(error),
            Self::Usage(_) => None,
        }
    }
}

impl From<io::Error> for CliError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<MeshyError> for CliError {
    fn from(value: MeshyError) -> Self {
        Self::Meshy(value)
    }
}

pub fn exit_on_error(result: Result<(), CliError>) {
    if let Err(error) = result {
        eprintln!("dd-meshy-adapter: {error}");
        process::exit(1);
    }
}
