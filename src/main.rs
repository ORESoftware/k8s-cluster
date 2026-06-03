use std::{
    collections::{HashMap, VecDeque},
    env,
    error::Error,
    ffi::{c_int, c_void},
    net::SocketAddr,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path as FsPath,
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    DD_REMOTE_MIP_SOLVER_STREAM_NAME, DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS,
    MIP_SOLVER_CONTROL_SUBJECT, MIP_SOLVER_EVENTS_SUBJECT, MIP_SOLVER_JOBS_SUBJECT,
    MIP_SOLVER_RESULTS_SUBJECT, MIP_SOLVER_WORKERS_QUEUE_GROUP,
};
use des_engine::des::general::{
    ip_mip_des::{
        solve_ipmip_with_des, BranchRule, ConcreteLpRelaxationAlgorithm, IPMIPProblem,
        IPMIPSolveOptions, IPMIPStatus, LpRelaxationAlgorithm,
    },
    lp::{solve_lp_internal, InternalSimplexOptions, LPProblem, LPStatus, Sense},
};
use futures_util::StreamExt;
use libloading::Library;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

const SERVICE_NAME: &str = "dd-in-house-mip-solver-node";
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_VARS: usize = 10_000;
const MAX_CONSTRAINTS: usize = 50_000;
const MAX_STREAM_COMMANDS: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum NodeRole {
    Master,
    Slave,
}

impl NodeRole {
    fn from_env() -> Self {
        match env_value("MIP_SOLVER_NODE_ROLE", "master")
            .to_ascii_lowercase()
            .as_str()
        {
            "slave" | "worker" => NodeRole::Slave,
            _ => NodeRole::Master,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            NodeRole::Master => "master",
            NodeRole::Slave => "slave",
        }
    }
}

#[derive(Clone)]
struct AppState {
    role: NodeRole,
    node_id: String,
    nats: Option<async_nats::Client>,
    jobs_subject: String,
    results_subject: String,
    control_subject: String,
    events_subject: String,
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    stream_events_total: AtomicU64,
    solve_requests_total: AtomicU64,
    subproblem_jobs_published_total: AtomicU64,
    subproblem_jobs_completed_total: AtomicU64,
    slave_jobs_processed_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Clone)]
struct LiveSession {
    problem: Option<MipProblemSpec>,
    revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveHttpRequest {
    request_id: Option<String>,
    problem: Option<MipProblemSpec>,
    commands: Option<Vec<Value>>,
    options: Option<SolveOptions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MipProblemSpec {
    #[serde(default = "default_sense")]
    sense: String,
    c: Vec<f64>,
    #[serde(rename = "a", alias = "A")]
    a: Vec<Vec<f64>>,
    b: Vec<f64>,
    #[serde(default)]
    integer_vars: Vec<bool>,
    ub: Option<Vec<f64>>,
    var_names: Option<Vec<String>>,
    con_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchConstraint {
    coefs: Vec<f64>,
    rhs: f64,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveOptions {
    max_nodes: Option<usize>,
    max_ticks: Option<usize>,
    lp_max_iters: Option<usize>,
    int_tol: Option<f64>,
    split_depth: Option<usize>,
    timeout_ms: Option<u64>,
    emit_trace: Option<bool>,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions {
            max_nodes: Some(20_000),
            max_ticks: Some(200_000),
            lp_max_iters: Some(5_000),
            int_tol: Some(1e-6),
            split_depth: Some(1),
            timeout_ms: Some(120_000),
            emit_trace: Some(false),
        }
    }
}

impl SolveOptions {
    fn merged(input: Option<SolveOptions>) -> Self {
        let defaults = SolveOptions::default();
        let Some(input) = input else {
            return defaults;
        };
        SolveOptions {
            max_nodes: input.max_nodes.or(defaults.max_nodes),
            max_ticks: input.max_ticks.or(defaults.max_ticks),
            lp_max_iters: input.lp_max_iters.or(defaults.lp_max_iters),
            int_tol: input.int_tol.or(defaults.int_tol),
            split_depth: input.split_depth.or(defaults.split_depth),
            timeout_ms: input.timeout_ms.or(defaults.timeout_ms),
            emit_trace: input.emit_trace.or(defaults.emit_trace),
        }
    }

    fn to_ipmip_options(&self) -> IPMIPSolveOptions {
        IPMIPSolveOptions {
            max_nodes: self.max_nodes,
            max_ticks: self.max_ticks,
            lp_max_iters: self.lp_max_iters,
            int_tol: self.int_tol,
            branch_rule: Some(BranchRule::MostFractional),
            lp_algorithm: Some(LpRelaxationAlgorithm::Concrete(
                ConcreteLpRelaxationAlgorithm::InternalSimplex,
            )),
            allow_external_solvers: Some(false),
            max_cut_rounds: Some(8),
            max_cuts_per_node: Some(16),
            heuristic_passes: Some(2),
            verbose: Some(false),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubproblemJob {
    solve_id: String,
    request_id: String,
    job_id: String,
    revision: u64,
    depth: usize,
    master_node: String,
    problem: MipProblemSpec,
    extra_constraints: Vec<BranchConstraint>,
    options: SolveOptions,
    submitted_at_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubproblemResult {
    solve_id: String,
    request_id: String,
    job_id: String,
    revision: u64,
    worker_node: String,
    ok: bool,
    status: String,
    z: Option<f64>,
    x: Vec<f64>,
    best_bound: Option<f64>,
    gap: Option<f64>,
    nodes_explored: usize,
    lp_solves: usize,
    elapsed_ms: f64,
    #[serde(default)]
    accelerator: AcceleratorReport,
    error: Option<String>,
    finished_at_ms: u128,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceleratorReport {
    mode: String,
    backend: String,
    gpu_available: bool,
    used_gpu: bool,
    used_cpu_parallel: bool,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveResponse {
    ok: bool,
    solve_id: String,
    request_id: String,
    status: String,
    revision: u64,
    z: Option<f64>,
    x: Vec<f64>,
    best_bound: Option<f64>,
    gap: Option<f64>,
    jobs_published: usize,
    jobs_completed: usize,
    timed_out: bool,
    distributed: bool,
    node_id: String,
    role: NodeRole,
    gpu: GpuStatus,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuStatus {
    available: bool,
    backend: String,
    used: bool,
    mode: String,
    note: Option<String>,
}

#[derive(Debug)]
struct BoundPreprocess {
    infeasible_reason: Option<String>,
    accelerator: AcceleratorReport,
}

#[derive(Debug)]
struct FrontierNode {
    depth: usize,
    extra_constraints: Vec<BranchConstraint>,
}

#[derive(Debug)]
struct LpRelaxation {
    status: LPStatus,
    x: Vec<f64>,
}

fn default_sense() -> String {
    "max".to_string()
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<String>) -> String {
    input
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("mip-{}", Uuid::new_v4()))
}

fn gpu_mode() -> String {
    env_value("MIP_SOLVER_GPU_MODE", "auto").to_ascii_lowercase()
}

fn gpu_available() -> bool {
    let visible = env::var("NVIDIA_VISIBLE_DEVICES")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "void" && value != "none");
    let device = FsPath::new("/dev/nvidia0").exists();
    visible.is_some() || device
}

fn gpu_status() -> GpuStatus {
    gpu_status_from_report(&AcceleratorReport::runtime())
}

fn gpu_status_from_report(report: &AcceleratorReport) -> GpuStatus {
    let note = report.notes.first().cloned();
    GpuStatus {
        available: report.gpu_available,
        backend: report.backend.clone(),
        used: report.used_gpu,
        mode: report.mode.clone(),
        note,
    }
}

fn aggregate_gpu_status(results: &[SubproblemResult]) -> GpuStatus {
    let mut report = AcceleratorReport::runtime();
    for result in results {
        report.merge(&result.accelerator);
    }
    gpu_status_from_report(&report)
}

impl AcceleratorReport {
    fn runtime() -> Self {
        let mode = gpu_mode();
        let gpu_available = gpu_available();
        AcceleratorReport {
            mode,
            backend: if gpu_available {
                "cuda-visible".to_string()
            } else {
                "cpu".to_string()
            },
            gpu_available,
            used_gpu: false,
            used_cpu_parallel: false,
            notes: Vec::new(),
        }
    }

    fn for_mode(mode: &str) -> Self {
        let gpu_available = gpu_available();
        AcceleratorReport {
            mode: mode.to_ascii_lowercase(),
            backend: if gpu_available {
                "cuda-visible".to_string()
            } else {
                "cpu".to_string()
            },
            gpu_available,
            used_gpu: false,
            used_cpu_parallel: false,
            notes: Vec::new(),
        }
    }

    fn merge(&mut self, other: &AcceleratorReport) {
        self.gpu_available |= other.gpu_available;
        self.used_gpu |= other.used_gpu;
        self.used_cpu_parallel |= other.used_cpu_parallel;
        if other.used_gpu || other.backend != "cpu" {
            self.backend = other.backend.clone();
        }
        for note in &other.notes {
            if !self.notes.iter().any(|existing| existing == note) {
                self.notes.push(note.clone());
            }
        }
    }
}

fn gpu_disabled(mode: &str) -> bool {
    matches!(mode, "off" | "false" | "0" | "disabled" | "none")
}

fn gpu_required(mode: &str) -> bool {
    matches!(mode, "require" | "required" | "must")
}

fn dense_matvec_cpu(a: &[Vec<f64>], x: &[f64]) -> (Vec<f64>, bool) {
    let rows = a.len();
    if rows == 0 {
        return (Vec::new(), false);
    }
    let workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows);
    if workers <= 1 || rows < 256 {
        let values = a
            .iter()
            .map(|row| {
                row.iter()
                    .zip(x.iter())
                    .map(|(coef, value)| coef * value)
                    .sum()
            })
            .collect();
        return (values, false);
    }

    let chunk_len = rows.div_ceil(workers);
    let mut values = vec![0.0; rows];
    thread::scope(|scope| {
        for (chunk_index, out) in values.chunks_mut(chunk_len).enumerate() {
            let start = chunk_index * chunk_len;
            let input = &a[start..start + out.len()];
            scope.spawn(move || {
                for (slot, row) in out.iter_mut().zip(input.iter()) {
                    *slot = row
                        .iter()
                        .zip(x.iter())
                        .map(|(coef, value)| coef * value)
                        .sum();
                }
            });
        }
    });
    (values, true)
}

fn dense_matvec_accelerated_with_mode(
    a: &[Vec<f64>],
    x: &[f64],
    mode: &str,
) -> Result<(Vec<f64>, AcceleratorReport), String> {
    let mut report = AcceleratorReport::for_mode(mode);
    if a.is_empty() {
        return Ok((Vec::new(), report));
    }
    if a.iter().any(|row| row.len() != x.len()) {
        return Err("accelerated matvec received a non-rectangular matrix".to_string());
    }

    if !gpu_disabled(&report.mode) {
        if report.gpu_available {
            match dense_matvec_cuda_row_major(a, x) {
                Ok(values) => {
                    report.backend = "cuda-cublas-dgemv".to_string();
                    report.used_gpu = true;
                    return Ok((values, report));
                }
                Err(error) if gpu_required(&report.mode) => return Err(error),
                Err(error) => report.notes.push(format!(
                    "CUDA/cuBLAS unavailable; used CPU fallback: {error}"
                )),
            }
        } else {
            let note = "GPU requested but no NVIDIA device is visible".to_string();
            if gpu_required(&report.mode) {
                return Err(note);
            }
            report.notes.push(note);
        }
    }

    let (values, used_parallel) = dense_matvec_cpu(a, x);
    report.backend = if used_parallel {
        "in-house-cpu-threaded".to_string()
    } else {
        "in-house-cpu".to_string()
    };
    report.used_cpu_parallel = used_parallel;
    Ok((values, report))
}

type CudaResult = c_int;
type CublasResult = c_int;
type CublasHandle = *mut c_void;

const CUDA_SUCCESS: CudaResult = 0;
const CUBLAS_STATUS_SUCCESS: CublasResult = 0;
const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;
const CUBLAS_OP_T: c_int = 1;

struct CudaLibraries {
    _cudart: Library,
    _cublas: Library,
    cuda_malloc: unsafe extern "C" fn(*mut *mut c_void, usize) -> CudaResult,
    cuda_free: unsafe extern "C" fn(*mut c_void) -> CudaResult,
    cuda_memcpy: unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> CudaResult,
    cuda_device_synchronize: unsafe extern "C" fn() -> CudaResult,
    cublas_create: unsafe extern "C" fn(*mut CublasHandle) -> CublasResult,
    cublas_destroy: unsafe extern "C" fn(CublasHandle) -> CublasResult,
    cublas_dgemv: unsafe extern "C" fn(
        CublasHandle,
        c_int,
        c_int,
        c_int,
        *const f64,
        *const f64,
        c_int,
        *const f64,
        c_int,
        *const f64,
        *mut f64,
        c_int,
    ) -> CublasResult,
}

impl CudaLibraries {
    fn load() -> Result<Self, String> {
        let cudart = open_first_library(&["libcudart.so", "libcudart.so.12", "libcudart.so.11.0"])?;
        let cublas = open_first_library(&["libcublas.so", "libcublas.so.12", "libcublas.so.11"])?;
        unsafe {
            Ok(CudaLibraries {
                cuda_malloc: load_symbol(&cudart, b"cudaMalloc\0")?,
                cuda_free: load_symbol(&cudart, b"cudaFree\0")?,
                cuda_memcpy: load_symbol(&cudart, b"cudaMemcpy\0")?,
                cuda_device_synchronize: load_symbol(&cudart, b"cudaDeviceSynchronize\0")?,
                cublas_create: load_symbol(&cublas, b"cublasCreate_v2\0")?,
                cublas_destroy: load_symbol(&cublas, b"cublasDestroy_v2\0")?,
                cublas_dgemv: load_symbol(&cublas, b"cublasDgemv_v2\0")?,
                _cudart: cudart,
                _cublas: cublas,
            })
        }
    }
}

fn open_first_library(candidates: &[&str]) -> Result<Library, String> {
    let mut errors = Vec::new();
    for candidate in candidates {
        match unsafe { Library::new(candidate) } {
            Ok(library) => return Ok(library),
            Err(error) => errors.push(format!("{candidate}: {error}")),
        }
    }
    Err(errors.join("; "))
}

unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
    library
        .get::<T>(name)
        .map(|symbol| *symbol)
        .map_err(|error| {
            let label_bytes = name.strip_suffix(&[0]).unwrap_or(name);
            let label = String::from_utf8_lossy(label_bytes);
            format!("missing CUDA symbol {label}: {error}")
        })
}

struct CudaAllocation<'a> {
    libs: &'a CudaLibraries,
    ptr: *mut c_void,
}

impl<'a> CudaAllocation<'a> {
    fn new(libs: &'a CudaLibraries, bytes: usize) -> Result<Self, String> {
        let mut ptr = ptr::null_mut();
        check_cuda(unsafe { (libs.cuda_malloc)(&mut ptr, bytes) }, "cudaMalloc")?;
        Ok(CudaAllocation { libs, ptr })
    }

    fn copy_from_host<T>(&self, input: &[T]) -> Result<(), String> {
        check_cuda(
            unsafe {
                (self.libs.cuda_memcpy)(
                    self.ptr,
                    input.as_ptr() as *const c_void,
                    std::mem::size_of_val(input),
                    CUDA_MEMCPY_HOST_TO_DEVICE,
                )
            },
            "cudaMemcpy host-to-device",
        )
    }

    fn copy_to_host<T>(&self, output: &mut [T]) -> Result<(), String> {
        check_cuda(
            unsafe {
                (self.libs.cuda_memcpy)(
                    output.as_mut_ptr() as *mut c_void,
                    self.ptr,
                    std::mem::size_of_val(output),
                    CUDA_MEMCPY_DEVICE_TO_HOST,
                )
            },
            "cudaMemcpy device-to-host",
        )
    }
}

impl Drop for CudaAllocation<'_> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { (self.libs.cuda_free)(self.ptr) };
        }
    }
}

struct CublasContext<'a> {
    libs: &'a CudaLibraries,
    handle: CublasHandle,
}

impl<'a> CublasContext<'a> {
    fn new(libs: &'a CudaLibraries) -> Result<Self, String> {
        let mut handle = ptr::null_mut();
        check_cublas(
            unsafe { (libs.cublas_create)(&mut handle) },
            "cublasCreate_v2",
        )?;
        Ok(CublasContext { libs, handle })
    }
}

impl Drop for CublasContext<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let _ = unsafe { (self.libs.cublas_destroy)(self.handle) };
        }
    }
}

fn check_cuda(code: CudaResult, op: &str) -> Result<(), String> {
    if code == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(format!("{op} failed with CUDA status {code}"))
    }
}

fn check_cublas(code: CublasResult, op: &str) -> Result<(), String> {
    if code == CUBLAS_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(format!("{op} failed with cuBLAS status {code}"))
    }
}

fn dense_matvec_cuda_row_major(a: &[Vec<f64>], x: &[f64]) -> Result<Vec<f64>, String> {
    let rows = a.len();
    let cols = x.len();
    if rows == 0 {
        return Ok(Vec::new());
    }
    if rows > c_int::MAX as usize || cols > c_int::MAX as usize {
        return Err("matrix is too large for cuBLAS int dimensions".to_string());
    }
    let libs = CudaLibraries::load()?;
    let cublas = CublasContext::new(&libs)?;
    let flat: Vec<f64> = a.iter().flat_map(|row| row.iter().copied()).collect();
    let mut output = vec![0.0; rows];

    let d_a = CudaAllocation::new(&libs, std::mem::size_of_val(flat.as_slice()))?;
    let d_x = CudaAllocation::new(&libs, std::mem::size_of_val(x))?;
    let d_y = CudaAllocation::new(&libs, std::mem::size_of_val(output.as_slice()))?;
    d_a.copy_from_host(&flat)?;
    d_x.copy_from_host(x)?;

    let alpha = 1.0;
    let beta = 0.0;
    check_cublas(
        unsafe {
            (libs.cublas_dgemv)(
                cublas.handle,
                CUBLAS_OP_T,
                cols as c_int,
                rows as c_int,
                &alpha,
                d_a.ptr as *const f64,
                cols as c_int,
                d_x.ptr as *const f64,
                1,
                &beta,
                d_y.ptr as *mut f64,
                1,
            )
        },
        "cublasDgemv_v2",
    )?;
    check_cuda(
        unsafe { (libs.cuda_device_synchronize)() },
        "cudaDeviceSynchronize",
    )?;
    d_y.copy_to_host(&mut output)?;
    Ok(output)
}

fn preprocess_bounds(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<BoundPreprocess, String> {
    preprocess_bounds_with_mode(problem, extra_constraints, &gpu_mode())
}

fn preprocess_bounds_with_mode(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
    mode: &str,
) -> Result<BoundPreprocess, String> {
    let problem = normalized_problem(problem.clone())?;
    let n = problem.c.len();
    let mut rows = problem.a.clone();
    let mut rhs = problem.b.clone();
    for constraint in extra_constraints {
        if constraint.coefs.len() != n {
            return Err(format!(
                "branch constraint {} has length {}, expected {}",
                constraint.name,
                constraint.coefs.len(),
                n
            ));
        }
        rows.push(constraint.coefs.clone());
        rhs.push(constraint.rhs);
    }

    let upper_bounds = problem.ub.clone().unwrap_or_else(|| vec![f64::INFINITY; n]);
    let finite_ub: Vec<f64> = upper_bounds
        .iter()
        .map(|ub| if ub.is_finite() { *ub } else { 0.0 })
        .collect();
    let mut positive = vec![vec![0.0; n]; rows.len()];
    let mut negative = vec![vec![0.0; n]; rows.len()];
    let mut positive_infinite = vec![false; rows.len()];
    let mut negative_infinite = vec![false; rows.len()];

    for (row_index, row) in rows.iter().enumerate() {
        for (col, coef) in row.iter().enumerate() {
            if *coef > 0.0 {
                if upper_bounds[col].is_finite() {
                    positive[row_index][col] = *coef;
                } else {
                    positive_infinite[row_index] = true;
                }
            } else if *coef < 0.0 {
                if upper_bounds[col].is_finite() {
                    negative[row_index][col] = *coef;
                } else {
                    negative_infinite[row_index] = true;
                }
            }
        }
    }

    let (max_finite, mut accelerator) =
        dense_matvec_accelerated_with_mode(&positive, &finite_ub, mode)?;
    let (min_finite, negative_report) =
        dense_matvec_accelerated_with_mode(&negative, &finite_ub, mode)?;
    accelerator.merge(&negative_report);

    let mut always_satisfied_rows = 0usize;
    for row_index in 0..rows.len() {
        let min_activity = if negative_infinite[row_index] {
            f64::NEG_INFINITY
        } else {
            min_finite[row_index]
        };
        let max_activity = if positive_infinite[row_index] {
            f64::INFINITY
        } else {
            max_finite[row_index]
        };
        if min_activity > rhs[row_index] + 1e-9 {
            return Ok(BoundPreprocess {
                infeasible_reason: Some(format!(
                    "bound preprocessing proved row {row_index} infeasible: min activity {min_activity:.6} > rhs {:.6}",
                    rhs[row_index]
                )),
                accelerator,
            });
        }
        if max_activity.is_finite() && max_activity <= rhs[row_index] + 1e-9 {
            always_satisfied_rows += 1;
        }
    }
    if always_satisfied_rows > 0 {
        accelerator.notes.push(format!(
            "bound preprocessing found {always_satisfied_rows} rows always satisfied by variable bounds"
        ));
    }

    Ok(BoundPreprocess {
        infeasible_reason: None,
        accelerator,
    })
}

fn sense_of(raw: &str) -> Sense {
    match raw.to_ascii_lowercase().as_str() {
        "min" | "minimize" | "minimise" => Sense::Min,
        _ => Sense::Max,
    }
}

fn validate_problem(problem: &MipProblemSpec) -> Result<(), String> {
    let n = problem.c.len();
    if n == 0 {
        return Err("objective vector `c` must not be empty".to_string());
    }
    if n > MAX_VARS {
        return Err(format!("variable count {n} exceeds limit {MAX_VARS}"));
    }
    if problem.a.len() != problem.b.len() {
        return Err(format!(
            "`a` has {} rows but `b` has {} entries",
            problem.a.len(),
            problem.b.len()
        ));
    }
    if problem.a.len() > MAX_CONSTRAINTS {
        return Err(format!(
            "constraint count {} exceeds limit {MAX_CONSTRAINTS}",
            problem.a.len()
        ));
    }
    if problem.c.iter().any(|v| !v.is_finite()) {
        return Err("objective coefficients must be finite".to_string());
    }
    if problem.b.iter().any(|v| !v.is_finite()) {
        return Err("right-hand sides must be finite".to_string());
    }
    for (i, row) in problem.a.iter().enumerate() {
        if row.len() != n {
            return Err(format!("row {i} has length {}, expected {n}", row.len()));
        }
        if row.iter().any(|v| !v.is_finite()) {
            return Err(format!("row {i} contains a non-finite coefficient"));
        }
    }
    if problem.integer_vars.len() > n {
        return Err("integerVars length must not exceed len(c)".to_string());
    }
    if let Some(ub) = &problem.ub {
        if ub.len() != n {
            return Err("ub length must equal len(c)".to_string());
        }
        if ub.iter().any(|v| v.is_nan() || *v < 0.0) {
            return Err("ub entries must be non-negative or infinite".to_string());
        }
    }
    Ok(())
}

fn normalized_problem(mut problem: MipProblemSpec) -> Result<MipProblemSpec, String> {
    validate_problem(&problem)?;
    problem.integer_vars.resize(problem.c.len(), false);
    Ok(problem)
}

fn vec_f64(command: &Value, key: &str) -> Option<Vec<f64>> {
    command.get(key)?.as_array().map(|items| {
        items
            .iter()
            .map(|value| value.as_f64().unwrap_or(0.0))
            .collect()
    })
}

fn vec_vec_f64(command: &Value, key: &str) -> Option<Vec<Vec<f64>>> {
    command.get(key)?.as_array().map(|rows| {
        rows.iter()
            .map(|row| {
                row.as_array()
                    .map(|cells| {
                        cells
                            .iter()
                            .map(|value| value.as_f64().unwrap_or(0.0))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect()
    })
}

fn usize_at(command: &Value, key: &str) -> Option<usize> {
    command.get(key).and_then(Value::as_u64).map(|v| v as usize)
}

fn f64_at(command: &Value, key: &str, fallback: f64) -> f64 {
    command.get(key).and_then(Value::as_f64).unwrap_or(fallback)
}

fn bool_at(command: &Value, key: &str, fallback: bool) -> bool {
    command
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

fn str_at(command: &Value, key: &str) -> Option<String> {
    command.get(key).and_then(Value::as_str).map(String::from)
}

fn parse_problem_from_commands(
    commands: &[Value],
) -> Result<(MipProblemSpec, u64, Vec<Value>), String> {
    if commands.len() > MAX_STREAM_COMMANDS {
        return Err(format!(
            "stream command count {} exceeds limit {MAX_STREAM_COMMANDS}",
            commands.len()
        ));
    }
    let mut problem: Option<MipProblemSpec> = None;
    let mut revision = 0;
    let mut frames = Vec::new();
    for command in commands {
        apply_stream_command(&mut problem, &mut revision, command, &mut frames)?;
    }
    let problem = problem.ok_or_else(|| {
        "no problem initialized; first command must be {\"op\":\"init\", ...}".to_string()
    })?;
    Ok((problem, revision, frames))
}

fn apply_stream_command(
    problem: &mut Option<MipProblemSpec>,
    revision: &mut u64,
    command: &Value,
    frames: &mut Vec<Value>,
) -> Result<(), String> {
    let op = command.get("op").and_then(Value::as_str).unwrap_or("");
    if op == "init" {
        let mut next = if let Some(raw) = command.get("problem") {
            serde_json::from_value::<MipProblemSpec>(raw.clone())
                .map_err(|err| format!("invalid problem: {err}"))?
        } else {
            MipProblemSpec {
                sense: str_at(command, "sense").unwrap_or_else(default_sense),
                c: vec_f64(command, "c").unwrap_or_default(),
                a: vec_vec_f64(command, "a").unwrap_or_default(),
                b: vec_f64(command, "b").unwrap_or_default(),
                integer_vars: command
                    .get("integerVars")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(|v| v.as_bool().unwrap_or(false)).collect())
                    .unwrap_or_default(),
                ub: vec_f64(command, "ub"),
                var_names: None,
                con_names: None,
            }
        };
        next = normalized_problem(next)?;
        *problem = Some(next);
        *revision += 1;
        frames.push(json!({"event":"initialized","revision":revision}));
        return Ok(());
    }

    let p = problem
        .as_mut()
        .ok_or_else(|| "no problem initialized; send init first".to_string())?;
    match op {
        "add_constraint" => {
            let coefs = vec_f64(command, "coefs").unwrap_or_default();
            if coefs.len() != p.c.len() {
                return Err("coefs length must equal variable count".to_string());
            }
            let rhs = f64_at(command, "rhs", 0.0);
            if !rhs.is_finite() {
                return Err("rhs must be finite".to_string());
            }
            p.a.push(coefs);
            p.b.push(rhs);
        }
        "set_constraint" | "modify_constraint" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.a.len() {
                return Err("constraint index out of range".to_string());
            }
            if let Some(coefs) = vec_f64(command, "coefs") {
                if coefs.len() != p.c.len() {
                    return Err("coefs length must equal variable count".to_string());
                }
                p.a[index] = coefs;
            }
            if command.get("rhs").is_some() {
                let rhs = f64_at(command, "rhs", p.b[index]);
                if !rhs.is_finite() {
                    return Err("rhs must be finite".to_string());
                }
                p.b[index] = rhs;
            }
        }
        "remove_constraint" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.a.len() {
                return Err("constraint index out of range".to_string());
            }
            p.a.remove(index);
            p.b.remove(index);
            if let Some(names) = p.con_names.as_mut() {
                names.remove(index);
            }
        }
        "set_rhs" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.b.len() {
                return Err("constraint index out of range".to_string());
            }
            let rhs = f64_at(command, "rhs", p.b[index]);
            if !rhs.is_finite() {
                return Err("rhs must be finite".to_string());
            }
            p.b[index] = rhs;
        }
        "set_coefficient" => {
            let row = usize_at(command, "row").ok_or("row is required")?;
            let col = usize_at(command, "col").ok_or("col is required")?;
            if row >= p.a.len() || col >= p.c.len() {
                return Err("coefficient index out of range".to_string());
            }
            p.a[row][col] = f64_at(command, "value", p.a[row][col]);
        }
        "add_variable" => {
            let column = vec_f64(command, "column").unwrap_or_else(|| vec![0.0; p.a.len()]);
            if column.len() != p.a.len() {
                return Err("column length must equal constraint count".to_string());
            }
            p.c.push(f64_at(command, "c", 0.0));
            p.integer_vars.push(bool_at(command, "integer", false));
            for (row, value) in p.a.iter_mut().zip(column.iter()) {
                row.push(*value);
            }
            if p.ub.is_some() || command.get("ub").is_some() {
                let upper = f64_at(command, "ub", f64::INFINITY);
                p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len() - 1])
                    .push(upper);
            }
        }
        "set_variable" | "modify_variable" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            if command.get("c").is_some() {
                p.c[index] = f64_at(command, "c", p.c[index]);
            }
            if command.get("integer").is_some() {
                p.integer_vars[index] = bool_at(command, "integer", p.integer_vars[index]);
            }
            if command.get("ub").is_some() {
                p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len()])[index] =
                    f64_at(command, "ub", f64::INFINITY);
            }
            if let Some(column) = vec_f64(command, "column") {
                if column.len() != p.a.len() {
                    return Err("column length must equal constraint count".to_string());
                }
                for (row, value) in p.a.iter_mut().zip(column.iter()) {
                    row[index] = *value;
                }
            }
        }
        "remove_variable" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            if p.c.len() == 1 {
                return Err("cannot remove the last variable".to_string());
            }
            p.c.remove(index);
            p.integer_vars.remove(index);
            for row in &mut p.a {
                row.remove(index);
            }
            if let Some(ub) = p.ub.as_mut() {
                ub.remove(index);
            }
        }
        "set_objective" => {
            let c = vec_f64(command, "c").unwrap_or_default();
            if c.len() != p.c.len() {
                return Err("c length must equal variable count".to_string());
            }
            p.c = c;
        }
        "set_integer" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.integer_vars.len() {
                return Err("variable index out of range".to_string());
            }
            p.integer_vars[index] = bool_at(command, "integer", true);
        }
        "set_upper_bound" | "set_ub" => {
            let index = usize_at(command, "index").ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len()])[index] =
                f64_at(command, "ub", f64::INFINITY);
        }
        "set_sense" => {
            p.sense = str_at(command, "sense").unwrap_or_else(default_sense);
        }
        "snapshot" => {
            frames.push(json!({
                "event":"model",
                "revision": revision,
                "numVars": p.c.len(),
                "numConstraints": p.a.len(),
                "integerVars": p.integer_vars,
            }));
            return Ok(());
        }
        other => return Err(format!("unknown stream op `{other}`")),
    }
    *revision += 1;
    validate_problem(p)?;
    frames.push(json!({"event":"applied","op":op,"revision":revision}));
    Ok(())
}

fn to_ipmip_problem(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<IPMIPProblem, String> {
    let problem = normalized_problem(problem.clone())?;
    let mut a = problem.a.clone();
    let mut b = problem.b.clone();
    for constraint in extra_constraints {
        if constraint.coefs.len() != problem.c.len() {
            return Err(format!(
                "branch constraint {} has length {}, expected {}",
                constraint.name,
                constraint.coefs.len(),
                problem.c.len()
            ));
        }
        a.push(constraint.coefs.clone());
        b.push(constraint.rhs);
    }
    Ok(IPMIPProblem {
        sense: sense_of(&problem.sense),
        c: problem.c,
        a,
        b,
        integer_vars: problem.integer_vars,
        ub: problem.ub,
        var_names: problem.var_names,
        con_names: problem.con_names,
        lazy_constraints: None,
        variable_nodes: None,
        constraint_nodes: None,
    })
}

fn to_lp_problem(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<LPProblem, String> {
    let problem = normalized_problem(problem.clone())?;
    let mut a = problem.a.clone();
    let mut b = problem.b.clone();
    for constraint in extra_constraints {
        a.push(constraint.coefs.clone());
        b.push(constraint.rhs);
    }
    Ok(LPProblem {
        sense: sense_of(&problem.sense),
        c: problem.c.clone(),
        a_ub: Some(a),
        b_ub: Some(b),
        a_eq: None,
        b_eq: None,
        lb: Some(vec![Some(0.0); problem.c.len()]),
        ub: problem
            .ub
            .map(|ub| ub.into_iter().map(|v| v.is_finite().then_some(v)).collect()),
        var_names: problem.var_names.clone(),
        con_names: problem.con_names.clone(),
    })
}

fn solve_lp_relaxation(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
    lp_max_iters: usize,
) -> Result<LpRelaxation, String> {
    let lp = to_lp_problem(problem, extra_constraints)?;
    let sol = solve_lp_internal(
        &lp,
        &InternalSimplexOptions {
            max_iter: Some(lp_max_iters),
            tol: Some(1e-9),
        },
    );
    Ok(LpRelaxation {
        status: sol.status,
        x: sol.x,
    })
}

fn first_fractional(problem: &MipProblemSpec, x: &[f64], int_tol: f64) -> Option<(usize, f64)> {
    problem
        .integer_vars
        .iter()
        .enumerate()
        .filter(|(index, integer)| **integer && *index < x.len())
        .map(|(index, _)| (index, x[index]))
        .find(|(_, value)| (value - value.round()).abs() > int_tol)
}

fn branch_constraints(var: usize, value: f64, n: usize, depth: usize) -> [BranchConstraint; 2] {
    let floor = value.floor();
    let ceil = value.ceil();
    let mut left = vec![0.0; n];
    left[var] = 1.0;
    let mut right = vec![0.0; n];
    right[var] = -1.0;
    [
        BranchConstraint {
            coefs: left,
            rhs: floor,
            name: format!("branch_d{depth}_x{var}_le_{floor:.0}"),
        },
        BranchConstraint {
            coefs: right,
            rhs: -ceil,
            name: format!("branch_d{depth}_x{var}_ge_{ceil:.0}"),
        },
    ]
}

fn build_frontier_jobs(
    problem: &MipProblemSpec,
    solve_id: &str,
    request_id: &str,
    revision: u64,
    master_node: &str,
    options: &SolveOptions,
) -> Result<(Vec<SubproblemJob>, Vec<String>), String> {
    let split_depth = options.split_depth.unwrap_or(1).min(8);
    let lp_max_iters = options.lp_max_iters.unwrap_or(5_000);
    let int_tol = options.int_tol.unwrap_or(1e-6);
    let mut warnings = Vec::new();
    let mut queue = VecDeque::from([FrontierNode {
        depth: 0,
        extra_constraints: Vec::new(),
    }]);
    let mut jobs = Vec::new();

    while let Some(node) = queue.pop_front() {
        let relaxation = solve_lp_relaxation(problem, &node.extra_constraints, lp_max_iters)?;
        match relaxation.status {
            LPStatus::Infeasible => continue,
            LPStatus::NumericalError | LPStatus::IterLimit => {
                warnings.push(format!(
                    "LP relaxation at depth {} returned {}; keeping it as a subtree job",
                    node.depth,
                    relaxation.status.as_str()
                ));
            }
            LPStatus::Unbounded => {
                warnings.push(format!(
                    "LP relaxation at depth {} is unbounded; keeping it as a subtree job",
                    node.depth
                ));
            }
            LPStatus::Optimal => {}
        }

        if relaxation.status == LPStatus::Optimal && node.depth < split_depth {
            if let Some((var, value)) = first_fractional(problem, &relaxation.x, int_tol) {
                let [left, right] = branch_constraints(var, value, problem.c.len(), node.depth);
                let mut left_constraints = node.extra_constraints.clone();
                left_constraints.push(left);
                queue.push_back(FrontierNode {
                    depth: node.depth + 1,
                    extra_constraints: left_constraints,
                });
                let mut right_constraints = node.extra_constraints;
                right_constraints.push(right);
                queue.push_back(FrontierNode {
                    depth: node.depth + 1,
                    extra_constraints: right_constraints,
                });
                continue;
            }
        }

        let job_id = format!("{solve_id}-{}", jobs.len());
        jobs.push(SubproblemJob {
            solve_id: solve_id.to_string(),
            request_id: request_id.to_string(),
            job_id,
            revision,
            depth: node.depth,
            master_node: master_node.to_string(),
            problem: problem.clone(),
            extra_constraints: node.extra_constraints,
            options: options.clone(),
            submitted_at_ms: now_ms(),
        });
    }

    Ok((jobs, warnings))
}

fn solve_subproblem(job: SubproblemJob, worker_node: String) -> SubproblemResult {
    let started = Instant::now();
    let mut accelerator = AcceleratorReport::runtime();
    let mut pruned_reason = None;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let preprocess = preprocess_bounds(&job.problem, &job.extra_constraints)?;
        accelerator = preprocess.accelerator;
        if let Some(reason) = preprocess.infeasible_reason {
            pruned_reason = Some(reason);
            return Ok(None);
        }
        let problem = to_ipmip_problem(&job.problem, &job.extra_constraints)?;
        let solution = solve_ipmip_with_des(problem, job.options.to_ipmip_options());
        Ok::<_, String>(Some(solution))
    }));

    match result {
        Ok(Ok(Some(solution))) => SubproblemResult {
            solve_id: job.solve_id,
            request_id: job.request_id,
            job_id: job.job_id,
            revision: job.revision,
            worker_node,
            ok: solution.status == IPMIPStatus::Optimal || !solution.x.is_empty(),
            status: solution.status.as_str().to_string(),
            z: solution.z.is_finite().then_some(solution.z),
            x: solution.x,
            best_bound: solution
                .best_bound
                .is_finite()
                .then_some(solution.best_bound),
            gap: solution.gap.is_finite().then_some(solution.gap),
            nodes_explored: solution.nodes_explored,
            lp_solves: solution.lp_solves,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            accelerator,
            error: None,
            finished_at_ms: now_ms(),
        },
        Ok(Ok(None)) => infeasible_subproblem(
            job,
            worker_node,
            accelerator,
            pruned_reason.unwrap_or_else(|| "bound preprocessing proved infeasible".to_string()),
            started,
        ),
        Ok(Err(error)) => failed_subproblem(job, worker_node, accelerator, error, started),
        Err(_) => failed_subproblem(
            job,
            worker_node,
            accelerator,
            "solver panicked".to_string(),
            started,
        ),
    }
}

fn failed_subproblem(
    job: SubproblemJob,
    worker_node: String,
    accelerator: AcceleratorReport,
    error: String,
    started: Instant,
) -> SubproblemResult {
    SubproblemResult {
        solve_id: job.solve_id,
        request_id: job.request_id,
        job_id: job.job_id,
        revision: job.revision,
        worker_node,
        ok: false,
        status: "error".to_string(),
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        nodes_explored: 0,
        lp_solves: 0,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        accelerator,
        error: Some(error),
        finished_at_ms: now_ms(),
    }
}

fn infeasible_subproblem(
    job: SubproblemJob,
    worker_node: String,
    accelerator: AcceleratorReport,
    reason: String,
    started: Instant,
) -> SubproblemResult {
    SubproblemResult {
        solve_id: job.solve_id,
        request_id: job.request_id,
        job_id: job.job_id,
        revision: job.revision,
        worker_node,
        ok: false,
        status: "infeasible".to_string(),
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        nodes_explored: 0,
        lp_solves: 0,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        accelerator,
        error: Some(reason),
        finished_at_ms: now_ms(),
    }
}

fn aggregate_results(
    solve_id: String,
    request_id: String,
    revision: u64,
    problem: &MipProblemSpec,
    jobs_published: usize,
    results: Vec<SubproblemResult>,
    timed_out: bool,
    distributed: bool,
    state: &AppState,
    mut warnings: Vec<String>,
) -> SolveResponse {
    let maximize = sense_of(&problem.sense) == Sense::Max;
    let mut feasible: Vec<&SubproblemResult> = results
        .iter()
        .filter(|result| result.ok && result.z.is_some() && !result.x.is_empty())
        .collect();
    feasible.sort_by(|left, right| {
        let lz = left.z.unwrap_or(f64::NAN);
        let rz = right.z.unwrap_or(f64::NAN);
        if maximize {
            rz.total_cmp(&lz)
        } else {
            lz.total_cmp(&rz)
        }
    });
    let best = feasible.first().copied();
    let best_bound = if maximize {
        results.iter().filter_map(|r| r.best_bound).reduce(f64::max)
    } else {
        results.iter().filter_map(|r| r.best_bound).reduce(f64::min)
    };
    let z = best.and_then(|r| r.z);
    let gap = match (z, best_bound) {
        (Some(z), Some(bound)) => Some((bound - z).abs() / 1.0_f64.max(z.abs())),
        _ => None,
    };
    if timed_out {
        warnings.push("solve timed out before every subproblem result returned".to_string());
    }
    let all_finished = results.len() == jobs_published && !timed_out;
    let all_optimal = all_finished && results.iter().all(|result| result.status == "optimal");
    let status = if best.is_some() && all_optimal {
        "optimal"
    } else if best.is_some() {
        "feasible-partial"
    } else if results.iter().any(|result| result.status == "unbounded") {
        "unbounded"
    } else if timed_out {
        "timeout"
    } else {
        "infeasible"
    };

    SolveResponse {
        ok: best.is_some() || status == "infeasible",
        solve_id,
        request_id,
        status: status.to_string(),
        revision,
        z,
        x: best.map(|r| r.x.clone()).unwrap_or_default(),
        best_bound,
        gap,
        jobs_published,
        jobs_completed: results.len(),
        timed_out,
        distributed,
        node_id: state.node_id.clone(),
        role: state.role,
        gpu: aggregate_gpu_status(&results),
        warnings,
        generated_at_ms: now_ms(),
    }
}

async fn publish_event(state: &AppState, event_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let event = json!({
        "schema":"dd.mip-solver.event.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "role": state.role.as_str(),
        "eventName": event_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&event) {
        let _ = nats
            .publish(state.events_subject.clone(), bytes.into())
            .await;
    }
}

async fn publish_control(state: &AppState, command_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let command = json!({
        "schema":"dd.mip-solver.control.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "role": state.role.as_str(),
        "commandName": command_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&command) {
        let _ = nats
            .publish(state.control_subject.clone(), bytes.into())
            .await;
    }
}

fn mip_stream_config() -> async_nats::jetstream::stream::Config {
    async_nats::jetstream::stream::Config {
        name: DD_REMOTE_MIP_SOLVER_STREAM_NAME.to_string(),
        subjects: DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS
            .iter()
            .map(|subject| subject.to_string())
            .collect(),
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        max_age: Duration::from_secs(60 * 60 * 24 * 7),
        max_message_size: 8 * 1024 * 1024,
        ..Default::default()
    }
}

async fn ensure_mip_stream(
    client: async_nats::Client,
) -> Result<async_nats::jetstream::stream::Stream, Box<dyn Error + Send + Sync>> {
    let jetstream = async_nats::jetstream::new(client);
    Ok(jetstream.get_or_create_stream(mip_stream_config()).await?)
}

async fn jetstream_publish_ack(
    client: &async_nats::Client,
    subject: &str,
    payload: Vec<u8>,
) -> Result<u64, String> {
    let jetstream = async_nats::jetstream::new(client.clone());
    jetstream
        .get_or_create_stream(mip_stream_config())
        .await
        .map_err(|err| format!("ensure JetStream stream: {err}"))?;
    let ack = jetstream
        .publish(subject.to_string(), payload.into())
        .await
        .map_err(|err| format!("JetStream publish {subject}: {err}"))?
        .await
        .map_err(|err| format!("JetStream publish ack {subject}: {err}"))?;
    if ack.stream != DD_REMOTE_MIP_SOLVER_STREAM_NAME {
        return Err(format!(
            "JetStream ack for {subject} landed in stream {}, expected {}",
            ack.stream, DD_REMOTE_MIP_SOLVER_STREAM_NAME
        ));
    }
    Ok(ack.sequence)
}

async fn solve_problem_distributed(
    state: AppState,
    request_id: String,
    revision: u64,
    problem: MipProblemSpec,
    options: SolveOptions,
) -> Result<SolveResponse, String> {
    let problem = normalized_problem(problem)?;
    let solve_id = format!("solve-{}", Uuid::new_v4());
    let (jobs, mut warnings) = build_frontier_jobs(
        &problem,
        &solve_id,
        &request_id,
        revision,
        &state.node_id,
        &options,
    )?;
    if jobs.is_empty() {
        return Ok(aggregate_results(
            solve_id,
            request_id,
            revision,
            &problem,
            0,
            Vec::new(),
            false,
            false,
            &state,
            warnings,
        ));
    }

    let Some(nats) = state.nats.clone() else {
        let mut results = Vec::new();
        for job in jobs {
            let node = state.node_id.clone();
            let result = tokio::task::spawn_blocking(move || solve_subproblem(job, node))
                .await
                .map_err(|err| format!("local solve task failed: {err}"))?;
            results.push(result);
        }
        return Ok(aggregate_results(
            solve_id,
            request_id,
            revision,
            &problem,
            results.len(),
            results,
            false,
            false,
            &state,
            warnings,
        ));
    };

    let mut result_sub = nats
        .subscribe(state.results_subject.clone())
        .await
        .map_err(|err| format!("subscribe results: {err}"))?;

    publish_event(
        &state,
        "solve-frontier-built",
        json!({"solveId": &solve_id, "requestId": &request_id, "jobs": jobs.len()}),
    )
    .await;

    for job in &jobs {
        let payload = serde_json::to_vec(job).map_err(|err| format!("serialize job: {err}"))?;
        jetstream_publish_ack(&nats, &state.jobs_subject, payload).await?;
        state
            .metrics
            .subproblem_jobs_published_total
            .fetch_add(1, Ordering::Relaxed);
    }

    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(120_000));
    let deadline = Instant::now() + timeout;
    let mut results = Vec::new();
    let mut timed_out = false;
    while results.len() < jobs.len() {
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            break;
        }
        match tokio::time::timeout(deadline - now, result_sub.next()).await {
            Ok(Some(message)) => {
                let parsed = serde_json::from_slice::<SubproblemResult>(&message.payload).ok();
                if let Some(result) = parsed.filter(|result| result.solve_id == solve_id) {
                    results.push(result);
                    state
                        .metrics
                        .subproblem_jobs_completed_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(None) => {
                warnings.push("NATS result subscription closed".to_string());
                timed_out = true;
                break;
            }
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }

    let response = aggregate_results(
        solve_id,
        request_id,
        revision,
        &problem,
        jobs.len(),
        results,
        timed_out,
        true,
        &state,
        warnings,
    );
    publish_event(
        &state,
        "solve-finished",
        json!({
            "solveId": &response.solve_id,
            "requestId": &response.request_id,
            "status": &response.status,
            "jobsPublished": response.jobs_published,
            "jobsCompleted": response.jobs_completed,
            "timedOut": response.timed_out,
        }),
    )
    .await;
    Ok(response)
}

fn response_json<T: Serialize>(status: StatusCode, value: T) -> Response {
    (status, Json(value)).into_response()
}

async fn root(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "role": state.role.as_str(),
        "nodeId": state.node_id,
        "subjects": {
            "jobs": state.jobs_subject,
            "results": state.results_subject,
            "control": state.control_subject,
            "events": state.events_subject,
        },
        "stream": DD_REMOTE_MIP_SOLVER_STREAM_NAME,
        "queueGroup": MIP_SOLVER_WORKERS_QUEUE_GROUP,
        "gpu": gpu_status(),
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({"ok": true, "service": SERVICE_NAME}))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "role": state.role.as_str(),
        "nats": state.nats.is_some(),
    }))
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let m = &state.metrics;
    let body = format!(
        concat!(
            "dd_mip_solver_http_requests_total {}\n",
            "dd_mip_solver_stream_events_total {}\n",
            "dd_mip_solver_solve_requests_total {}\n",
            "dd_mip_solver_subproblem_jobs_published_total {}\n",
            "dd_mip_solver_subproblem_jobs_completed_total {}\n",
            "dd_mip_solver_slave_jobs_processed_total {}\n",
            "dd_mip_solver_errors_total {}\n"
        ),
        m.http_requests_total.load(Ordering::Relaxed),
        m.stream_events_total.load(Ordering::Relaxed),
        m.solve_requests_total.load(Ordering::Relaxed),
        m.subproblem_jobs_published_total.load(Ordering::Relaxed),
        m.subproblem_jobs_completed_total.load(Ordering::Relaxed),
        m.slave_jobs_processed_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    );
    ([("Content-Type", "text/plain; version=0.0.4")], body)
}

async fn example() -> impl IntoResponse {
    Json(json!({
        "requestId": "knapsack-demo",
        "problem": {
            "sense": "max",
            "c": [10.0, 40.0, 30.0, 50.0],
            "a": [[5.0, 4.0, 6.0, 3.0]],
            "b": [10.0],
            "integerVars": [true, true, true, true],
            "ub": [1.0, 1.0, 1.0, 1.0],
            "varNames": ["item0", "item1", "item2", "item3"]
        },
        "options": {
            "splitDepth": 2,
            "maxNodes": 10000,
            "timeoutMs": 120000
        }
    }))
}

async fn solve_http(
    State(state): State<AppState>,
    Json(input): Json<SolveHttpRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .solve_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not act as master"}),
        );
    }
    let request_id = request_id(input.request_id);
    let options = SolveOptions::merged(input.options);
    let (problem, revision) = if let Some(problem) = input.problem {
        (problem, 0)
    } else if let Some(commands) = input.commands {
        match parse_problem_from_commands(&commands) {
            Ok((problem, revision, _frames)) => (problem, revision),
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}));
            }
        }
    } else {
        return response_json(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"request needs either problem or commands"}),
        );
    };

    match solve_problem_distributed(state.clone(), request_id, revision, problem, options).await {
        Ok(response) => response_json(StatusCode::OK, response),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}))
        }
    }
}

async fn stream_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(input): Json<Value>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let commands = match input {
        Value::Array(items) => items,
        value => vec![value],
    };
    if commands.len() > MAX_STREAM_COMMANDS {
        return response_json(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"too many stream commands"}),
        );
    }
    let mut sessions = state.sessions.lock().expect("sessions mutex poisoned");
    let session = sessions.entry(session_id.clone()).or_insert(LiveSession {
        problem: None,
        revision: 0,
    });
    let mut frames = Vec::new();
    for command in &commands {
        if let Err(error) = apply_stream_command(
            &mut session.problem,
            &mut session.revision,
            command,
            &mut frames,
        ) {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            frames.push(json!({"event":"error","message":error,"revision":session.revision}));
        } else {
            state
                .metrics
                .stream_events_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    response_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "sessionId": session_id,
            "revision": session.revision,
            "frames": frames,
        }),
    )
}

async fn get_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    let sessions = state.sessions.lock().expect("sessions mutex poisoned");
    match sessions.get(&session_id) {
        Some(session) => response_json(
            StatusCode::OK,
            json!({
                "ok": true,
                "sessionId": session_id,
                "revision": session.revision,
                "problem": session.problem,
            }),
        ),
        None => response_json(
            StatusCode::NOT_FOUND,
            json!({"ok":false,"error":"session not found"}),
        ),
    }
}

async fn solve_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(input): Json<SolveHttpRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .solve_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not act as master"}),
        );
    }
    let (problem, revision) = {
        let sessions = state.sessions.lock().expect("sessions mutex poisoned");
        let Some(session) = sessions.get(&session_id) else {
            return response_json(
                StatusCode::NOT_FOUND,
                json!({"ok":false,"error":"session not found"}),
            );
        };
        let Some(problem) = session.problem.clone() else {
            return response_json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error":"session has no initialized problem"}),
            );
        };
        (problem, session.revision)
    };
    let request_id = request_id(input.request_id.or(Some(session_id)));
    let options = SolveOptions::merged(input.options);
    match solve_problem_distributed(state.clone(), request_id, revision, problem, options).await {
        Ok(response) => response_json(StatusCode::OK, response),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}))
        }
    }
}

async fn build_jetstream_consumer(
    client: async_nats::Client,
    consumer_name: &str,
    ack_wait: Duration,
    max_ack_pending: i64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let stream = ensure_mip_stream(client).await?;
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.to_string()),
                filter_subject: MIP_SOLVER_JOBS_SUBJECT.to_string(),
                ack_wait,
                max_ack_pending,
                max_deliver: 5,
                ..Default::default()
            },
        )
        .await?;
    Ok(consumer)
}

async fn run_slave(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(nats) = state.nats.clone() else {
        eprintln!("slave role requires NATS_URL");
        return Ok(());
    };
    let consumer_name = env_value("MIP_SOLVER_NATS_CONSUMER", MIP_SOLVER_WORKERS_QUEUE_GROUP);
    let ack_wait = Duration::from_secs(env_u64("MIP_SOLVER_ACK_WAIT_SECONDS", 600));
    let max_ack_pending = env_u64("MIP_SOLVER_MAX_ACK_PENDING", 32) as i64;
    let consumer =
        build_jetstream_consumer(nats.clone(), &consumer_name, ack_wait, max_ack_pending).await?;
    let mut messages = consumer.messages().await?;
    publish_event(
        &state,
        "slave-started",
        json!({"consumer": &consumer_name, "jobsSubject": &state.jobs_subject}),
    )
    .await;
    publish_control(
        &state,
        "worker-ready",
        json!({
            "consumer": &consumer_name,
            "jobsSubject": &state.jobs_subject,
            "resultsSubject": &state.results_subject,
        }),
    )
    .await;

    while let Some(message) = messages.next().await {
        publish_control(
            &state,
            "request-work",
            json!({"consumer": &consumer_name, "jobsSubject": &state.jobs_subject}),
        )
        .await;
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                eprintln!("mip solver worker message fetch failed: {error}");
                continue;
            }
        };
        let job = match serde_json::from_slice::<SubproblemJob>(&message.payload) {
            Ok(job) => job,
            Err(error) => {
                eprintln!("invalid mip solver job payload: {error}");
                let _ = message.ack().await;
                continue;
            }
        };
        let worker_node = state.node_id.clone();
        let result =
            match tokio::task::spawn_blocking(move || solve_subproblem(job, worker_node)).await {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("mip solver worker task failed: {error}");
                    let _ = message
                        .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                            Duration::from_secs(5),
                        )))
                        .await;
                    continue;
                }
            };
        let payload = serde_json::to_vec(&result)?;
        jetstream_publish_ack(&nats, &state.results_subject, payload)
            .await
            .map_err(|err| -> Box<dyn Error + Send + Sync> {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("publish subproblem result: {err}"),
                ))
            })?;
        publish_control(
            &state,
            "worker-completed",
            json!({
                "consumer": &consumer_name,
                "jobId": &result.job_id,
                "solveId": &result.solve_id,
                "status": &result.status,
                "resultsSubject": &state.results_subject,
            }),
        )
        .await;
        state
            .metrics
            .slave_jobs_processed_total
            .fetch_add(1, Ordering::Relaxed);
        if let Err(error) = message.ack().await {
            eprintln!("mip solver job ack failed: {error}");
        }
    }
    Ok(())
}

async fn connect_nats() -> Option<async_nats::Client> {
    let url = env::var("NATS_URL").ok()?;
    match async_nats::connect(url.clone()).await {
        Ok(client) => Some(client),
        Err(error) => {
            eprintln!("failed to connect to NATS at {url}: {error}");
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let role = NodeRole::from_env();
    let node_id = env::var("POD_NAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| format!("{}-{}", SERVICE_NAME, Uuid::new_v4()));
    let nats = connect_nats().await;
    let state = AppState {
        role,
        node_id,
        nats,
        jobs_subject: env_value("MIP_SOLVER_JOBS_SUBJECT", MIP_SOLVER_JOBS_SUBJECT),
        results_subject: env_value("MIP_SOLVER_RESULTS_SUBJECT", MIP_SOLVER_RESULTS_SUBJECT),
        control_subject: env_value("MIP_SOLVER_CONTROL_SUBJECT", MIP_SOLVER_CONTROL_SUBJECT),
        events_subject: env_value("MIP_SOLVER_EVENTS_SUBJECT", MIP_SOLVER_EVENTS_SUBJECT),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        metrics: Arc::new(Metrics::default()),
    };

    if state.role == NodeRole::Slave {
        let worker_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_slave(worker_state).await {
                eprintln!("mip solver slave loop stopped: {error}");
            }
        });
    }

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/model/example", get(example))
        .route("/solve", post(solve_http))
        .route("/sessions/:session_id", get(get_session))
        .route("/sessions/:session_id/events", post(stream_session))
        .route("/sessions/:session_id/solve", post(solve_session))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8097");
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("{SERVICE_NAME} listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn streaming_edits_update_live_problem_revision() {
        let commands = vec![
            json!({"op":"init","sense":"max","c":[3.0,2.0],"a":[[1.0,1.0]],"b":[4.0],"integerVars":[true,false]}),
            json!({"op":"set_rhs","index":0,"rhs":5.0}),
            json!({"op":"add_constraint","coefs":[2.0,1.0],"rhs":8.0}),
            json!({"op":"add_variable","c":4.0,"column":[0.0,1.0],"integer":true,"ub":3.0}),
            json!({"op":"set_variable","index":2,"c":5.0,"integer":true}),
            json!({"op":"remove_constraint","index":0}),
            json!({"op":"snapshot"}),
        ];
        let (problem, revision, frames) = parse_problem_from_commands(&commands).unwrap();
        assert_eq!(revision, 6);
        assert_eq!(problem.c, vec![3.0, 2.0, 5.0]);
        assert_eq!(problem.a, vec![vec![2.0, 1.0, 1.0]]);
        assert_eq!(problem.b, vec![8.0]);
        assert_eq!(problem.integer_vars, vec![true, false, true]);
        assert_eq!(problem.ub.as_ref().unwrap()[2], 3.0);
        assert!(frames
            .iter()
            .any(|frame| frame.get("event") == Some(&json!("model"))));
    }

    #[test]
    fn frontier_builder_splits_fractional_lp_relaxation() {
        let problem = normalized_problem(MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![1.5],
            integer_vars: vec![true],
            ub: None,
            var_names: None,
            con_names: None,
        })
        .unwrap();
        let options = SolveOptions {
            split_depth: Some(1),
            ..SolveOptions::default()
        };
        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-test",
            "request-test",
            7,
            "master-a",
            &options,
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(jobs.len(), 1);
        assert!(jobs.iter().all(|job| job.revision == 7));
        assert!(jobs.iter().all(|job| job.extra_constraints.len() == 1));
        assert_eq!(jobs[0].extra_constraints[0].coefs, vec![1.0]);
        assert_eq!(jobs[0].extra_constraints[0].rhs, 1.0);
        assert_eq!(jobs[0].depth, 1);
    }

    #[test]
    fn bound_preprocess_prunes_rows_impossible_under_lower_bounds() {
        let problem = MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![-1.0],
            integer_vars: vec![false],
            ub: Some(vec![10.0]),
            var_names: None,
            con_names: None,
        };

        let report = preprocess_bounds_with_mode(&problem, &[], "off").unwrap();

        assert!(report
            .infeasible_reason
            .as_deref()
            .unwrap_or_default()
            .contains("bound preprocessing proved row 0 infeasible"));
        assert_eq!(report.accelerator.backend, "in-house-cpu");
        assert!(!report.accelerator.used_gpu);
    }

    #[test]
    fn bound_preprocess_reports_rows_always_satisfied_by_bounds() {
        let problem = MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![5.0],
            integer_vars: vec![false],
            ub: Some(vec![3.0]),
            var_names: None,
            con_names: None,
        };

        let report = preprocess_bounds_with_mode(&problem, &[], "off").unwrap();

        assert!(report.infeasible_reason.is_none());
        assert!(report
            .accelerator
            .notes
            .iter()
            .any(|note| note.contains("rows always satisfied by variable bounds")));
    }

    #[test]
    fn solve_options_force_in_house_lp_and_mip_engines() {
        let options = SolveOptions::default().to_ipmip_options();

        assert_eq!(options.allow_external_solvers, Some(false));
        assert!(matches!(
            options.lp_algorithm,
            Some(LpRelaxationAlgorithm::Concrete(
                ConcreteLpRelaxationAlgorithm::InternalSimplex
            ))
        ));
        assert!(matches!(
            options.branch_rule,
            Some(BranchRule::MostFractional)
        ));
    }

    #[test]
    fn nats_subjects_are_generated_mip_solver_namespace() {
        let subjects = [
            MIP_SOLVER_JOBS_SUBJECT,
            MIP_SOLVER_RESULTS_SUBJECT,
            MIP_SOLVER_CONTROL_SUBJECT,
            MIP_SOLVER_EVENTS_SUBJECT,
        ];
        for subject in subjects {
            assert!(subject.starts_with("dd.remote.mip_solver."));
            assert!(DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS.contains(&subject));
        }

        let mut unique = subjects.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), subjects.len());
        assert_eq!(DD_REMOTE_MIP_SOLVER_STREAM_NAME, "DD_REMOTE_MIP_SOLVER");
        assert_eq!(
            MIP_SOLVER_WORKERS_QUEUE_GROUP,
            "dd-in-house-mip-solver-node-workers"
        );
    }

    #[test]
    fn jetstream_stream_config_contains_generated_subjects() {
        let config = mip_stream_config();

        assert_eq!(config.name, DD_REMOTE_MIP_SOLVER_STREAM_NAME);
        for subject in DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS {
            assert!(config
                .subjects
                .iter()
                .any(|configured| configured == subject));
        }
        assert_eq!(
            config.subjects.len(),
            DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS.len()
        );
    }
}
