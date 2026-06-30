//! Serve the msb-cloud HTTP contract on top of a local microsandbox backend.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        FromRef, FromRequestParts, Path, Query, State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, request::Parts},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post, put},
};
use bytes::Bytes;
use clap::Parser;
use futures::{SinkExt, StreamExt, stream};
use microsandbox::{
    Backend, LocalBackend, MicrosandboxError, MicrosandboxResult, Sandbox, SandboxConfig, Volume,
    VolumeConfig, VolumeHandle,
    logs::LogStreamOptions,
    sandbox::{FsEntry, FsEntryKind, FsMetadata, SandboxMetrics, SandboxStatus},
};
use microsandbox_protocol::{
    codec,
    exec::{ExecExited, ExecFailed, ExecRequest, ExecStarted, ExecStderr, ExecStdin, ExecStdout},
    message::{Message, MessageType},
};
use microsandbox_types::{
    CloudCreateSandboxRequest, CloudFsEntry, CloudFsEntryKind, CloudFsExistsResponse,
    CloudFsMetadata, CloudFsPathRequest, CloudFsTwoPathRequest, CloudMessageResponse,
    CloudPaginated, CloudSandbox, CloudSandboxMetrics, CloudSandboxStatus, CloudVolume, EnvVar,
    SandboxPolicy, SandboxResources, SandboxRuntimeOptions, SandboxSpec,
};
use microsandbox_utils::{BIN_SUBDIR, LIB_SUBDIR, libkrunfw_filename, msb_binary_filename};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

//--------------------------------------------------------------------------------------------------
// Constants
//--------------------------------------------------------------------------------------------------

const ORG_ID: &str = "local";
const CLOUD_EXEC_SUBPROTOCOL: &str = "msb.cbor";

//--------------------------------------------------------------------------------------------------
// Runtime paths
//--------------------------------------------------------------------------------------------------

fn configure_sdk_runtime_paths() {
    let Some(home) = runtime_home_candidate() else {
        return;
    };

    let msb_path = home
        .join(BIN_SUBDIR)
        .join(msb_binary_filename(std::env::consts::OS));
    if msb_path.is_file() {
        debug!(path = %msb_path.display(), "configured SDK msb runtime path");
        microsandbox::config::set_sdk_msb_path(msb_path);
    }

    let libkrunfw_path = home
        .join(LIB_SUBDIR)
        .join(libkrunfw_filename(std::env::consts::OS));
    if libkrunfw_path.is_file() {
        debug!(path = %libkrunfw_path.display(), "configured SDK libkrunfw runtime path");
        microsandbox::config::set_sdk_libkrunfw_path(libkrunfw_path);
    }
}

fn runtime_home_candidate() -> Option<PathBuf> {
    std::env::var_os("MSB_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".microsandbox")))
}

//--------------------------------------------------------------------------------------------------
// Types
//--------------------------------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Socket address to listen on.
    #[arg(long, env = "MSB_CLOUD_ADAPTER_BIND", default_value = "127.0.0.1:8088")]
    bind: SocketAddr,

    /// Bearer API key required by incoming SDK requests.
    #[arg(long, env = "MSB_CLOUD_ADAPTER_API_KEY")]
    api_key: String,
}

#[derive(Clone)]
struct AppState {
    api_key: Arc<str>,
    local: Arc<LocalBackend>,
}

struct Auth;

#[derive(Debug, Deserialize)]
struct CreateQuery {
    #[serde(default)]
    start: bool,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    #[serde(default)]
    sources: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FsPathQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct FsRemoveQuery {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Serialize)]
struct CloudLogPayload<'a> {
    source: &'a str,
    ts: chrono::DateTime<chrono::Utc>,
    text: String,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorDetails,
}

#[derive(Debug, Serialize)]
struct ErrorDetails {
    code: &'static str,
    message: String,
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

//--------------------------------------------------------------------------------------------------
// Main
//--------------------------------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    if args.api_key.trim().is_empty() {
        anyhow::bail!("MSB_CLOUD_ADAPTER_API_KEY or --api-key must be non-empty");
    }

    configure_sdk_runtime_paths();

    let local = Arc::new(LocalBackend::new().await?);
    microsandbox::set_default_backend(Arc::clone(&local) as Arc<dyn Backend>);

    let state = AppState {
        api_key: Arc::from(args.api_key),
        local,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/sandboxes", post(create_sandbox).get(list_sandboxes))
        .route(
            "/v1/sandboxes/by-name/{name}",
            get(get_sandbox_by_name).delete(remove_sandbox_by_name),
        )
        .route(
            "/v1/sandboxes/by-name/{name}/start",
            post(start_sandbox_by_name),
        )
        .route(
            "/v1/sandboxes/by-name/{name}/stop",
            post(stop_sandbox_by_name),
        )
        .route(
            "/v1/sandboxes/by-name/{name}/kill",
            post(kill_sandbox_by_name),
        )
        .route(
            "/v1/sandboxes/by-name/{name}/drain",
            post(drain_sandbox_by_name),
        )
        .route("/v1/sandboxes/{id}/logs", get(log_stream_by_id))
        .route("/v1/sandboxes/{id}/metrics", get(metrics_by_id))
        .route("/v1/sandboxes/{id}/fs/read", get(fs_read_by_id))
        .route("/v1/sandboxes/{id}/fs/write", put(fs_write_by_id))
        .route("/v1/sandboxes/{id}/fs/list", get(fs_list_by_id))
        .route("/v1/sandboxes/{id}/fs/stat", get(fs_stat_by_id))
        .route("/v1/sandboxes/{id}/fs/mkdir", post(fs_mkdir_by_id))
        .route("/v1/sandboxes/{id}/fs", delete(fs_remove_by_id))
        .route("/v1/sandboxes/{id}/fs/copy", post(fs_copy_by_id))
        .route("/v1/sandboxes/{id}/fs/rename", post(fs_rename_by_id))
        .route("/v1/sandboxes/{id}/fs/exists", get(fs_exists_by_id))
        .route("/v1/sandboxes/{id}/exec.cbor", get(exec_ws_by_id))
        .route("/v1/volumes", post(create_volume).get(list_volumes))
        .route(
            "/v1/volumes/{name}",
            get(get_volume_by_name).delete(remove_volume_by_name),
        )
        .route("/v1/volumes/{name}/fs/read", get(volume_fs_read))
        .route("/v1/volumes/{name}/fs/write", put(volume_fs_write))
        .route("/v1/volumes/{name}/fs/list", get(volume_fs_list))
        .route("/v1/volumes/{name}/fs/stat", get(volume_fs_stat))
        .route("/v1/volumes/{name}/fs/mkdir", post(volume_fs_mkdir))
        .route("/v1/volumes/{name}/fs", delete(volume_fs_remove))
        .route("/v1/volumes/{name}/fs/copy", post(volume_fs_copy))
        .route("/v1/volumes/{name}/fs/rename", post(volume_fs_rename))
        .route("/v1/volumes/{name}/fs/exists", get(volume_fs_exists))
        .with_state(state);

    info!(addr = %args.bind, "starting msb-cloud-adapter");
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

//--------------------------------------------------------------------------------------------------
// Routes
//--------------------------------------------------------------------------------------------------

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true}))
}

async fn create_sandbox(
    _auth: Auth,
    State(state): State<AppState>,
    Query(query): Query<CreateQuery>,
    Json(req): Json<CloudCreateSandboxRequest>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let config = sandbox_config_from_cloud_request(&req)?;
    let sandbox = with_local_backend(&state, Sandbox::create_detached(config)).await?;
    if !query.start {
        debug!(
            sandbox = %req.name,
            "local backend starts sandboxes immediately; accepted start=false as running"
        );
    }
    Ok(Json(cloud_sandbox_from_live(&sandbox, &req).await?))
}

async fn list_sandboxes(
    _auth: Auth,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<CloudPaginated<CloudSandbox>>, ApiError> {
    let handles = with_local_backend(&state, Sandbox::list()).await?;
    let mut data = Vec::with_capacity(handles.len());

    for handle in handles {
        if let Some(cursor) = &query.cursor
            && handle.name() <= cursor.as_str()
        {
            continue;
        }
        let req = cloud_request_from_handle(&handle)?;
        data.push(cloud_sandbox_from_handle(&handle, req));
        if let Some(limit) = query.limit
            && data.len() >= limit as usize
        {
            break;
        }
    }

    let next_cursor = query
        .limit
        .and_then(|limit| {
            (data.len() >= limit as usize).then(|| data.last().map(|s| s.name.clone()))
        })
        .flatten();

    Ok(Json(CloudPaginated { data, next_cursor }))
}

async fn get_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let handle = with_local_backend(&state, Sandbox::get(&name)).await?;
    let req = cloud_request_from_handle(&handle)?;
    Ok(Json(cloud_sandbox_from_handle(&handle, req)))
}

async fn start_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let sandbox = with_local_backend(&state, Sandbox::start_detached(&name)).await?;
    let req = cloud_request_from_config(sandbox.config())?;
    Ok(Json(cloud_sandbox_from_live(&sandbox, &req).await?))
}

async fn stop_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let handle = with_local_backend(&state, Sandbox::get(&name)).await?;
    let req = cloud_request_from_handle(&handle)?;
    if let Err(err) = handle.stop().await {
        if matches!(err, MicrosandboxError::SandboxNotFound(_)) && req.ephemeral {
            return Ok(Json(cloud_sandbox_terminal_from_handle(
                &handle,
                req,
                CloudSandboxStatus::Stopped,
            )));
        }
        return Err(ApiError::from(err));
    }
    Ok(Json(
        lifecycle_response_after_operation(
            &state,
            &name,
            &handle,
            req,
            CloudSandboxStatus::Stopped,
        )
        .await?,
    ))
}

async fn kill_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let handle = with_local_backend(&state, Sandbox::get(&name)).await?;
    let req = cloud_request_from_handle(&handle)?;
    if let Err(err) = handle.kill().await {
        if matches!(err, MicrosandboxError::SandboxNotFound(_)) && req.ephemeral {
            return Ok(Json(cloud_sandbox_terminal_from_handle(
                &handle,
                req,
                CloudSandboxStatus::Stopped,
            )));
        }
        return Err(ApiError::from(err));
    }
    Ok(Json(
        lifecycle_response_after_operation(
            &state,
            &name,
            &handle,
            req,
            CloudSandboxStatus::Stopped,
        )
        .await?,
    ))
}

async fn drain_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudSandbox>, ApiError> {
    let handle = with_local_backend(&state, Sandbox::get(&name)).await?;
    let req = cloud_request_from_handle(&handle)?;
    if let Err(err) = handle.request_drain().await {
        if matches!(err, MicrosandboxError::SandboxNotFound(_)) && req.ephemeral {
            return Ok(Json(cloud_sandbox_terminal_from_handle(
                &handle,
                req,
                CloudSandboxStatus::Stopped,
            )));
        }
        return Err(ApiError::from(err));
    }
    Ok(Json(
        lifecycle_response_after_operation(
            &state,
            &name,
            &handle,
            req,
            CloudSandboxStatus::Stopped,
        )
        .await?,
    ))
}

async fn remove_sandbox_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    match with_local_backend(&state, Sandbox::remove(&name)).await {
        Ok(()) => {}
        Err(err) if err.status == StatusCode::NOT_FOUND && err.code == "sandbox_not_found" => {
            return Ok(Json(CloudMessageResponse {
                message: format!("sandbox {name} already removed"),
            }));
        }
        Err(err) => return Err(err),
    }

    Ok(Json(CloudMessageResponse {
        message: format!("sandbox {name} removed"),
    }))
}

async fn log_stream_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    let name = sandbox_name_from_cloud_id(&id);
    let opts = LogStreamOptions {
        sources: parse_log_sources(query.sources.as_deref())?,
        follow: true,
        ..Default::default()
    };
    let logs = with_local_backend(&state, async {
        Sandbox::get(&name).await?.log_stream(&opts).await
    })
    .await?;

    let output = stream::unfold(
        (logs, 0u64, false),
        move |(mut logs, seq, done)| async move {
            if done {
                return None;
            }
            match logs.next().await {
                Some(Ok(entry)) => {
                    let source = log_source_name(entry.source);
                    let payload = CloudLogPayload {
                        source,
                        ts: entry.timestamp,
                        text: String::from_utf8_lossy(&entry.data).into_owned(),
                    };
                    match serde_json::to_string(&payload) {
                        Ok(data) => Some((
                            Ok(Event::default().id(seq.to_string()).data(data)),
                            (logs, seq + 1, false),
                        )),
                        Err(error) => Some((Err(error), (logs, seq + 1, false))),
                    }
                }
                Some(Err(error)) => {
                    let data = serde_json::json!({
                        "source": "system",
                        "ts": chrono::Utc::now(),
                        "text": error.to_string(),
                    })
                    .to_string();
                    Some((
                        Ok(Event::default()
                            .event("error")
                            .id(seq.to_string())
                            .data(data)),
                        (logs, seq + 1, false),
                    ))
                }
                None => Some((
                    Ok(Event::default().event("end").data("{}")),
                    (logs, seq + 1, true),
                )),
            }
        },
    );

    Ok(Sse::new(output)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn exec_ws_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    Ok(ws
        .protocols([CLOUD_EXEC_SUBPROTOCOL])
        .on_upgrade(move |socket| handle_exec_ws(state, sandbox_name_from_cloud_id(&id), socket))
        .into_response())
}

async fn metrics_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloudSandboxMetrics>, ApiError> {
    let name = sandbox_name_from_cloud_id(&id);
    let metrics =
        with_local_backend(&state, async { Sandbox::get(&name).await?.metrics().await }).await?;
    Ok(Json(cloud_metrics_from_sdk(metrics)))
}

async fn fs_read_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Bytes, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    Ok(sandbox.fs().read(&query.path).await?)
}

async fn fs_write_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsPathQuery>,
    body: Bytes,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    sandbox.fs().write(&query.path, body).await?;
    Ok(Json(CloudMessageResponse {
        message: "file written".into(),
    }))
}

async fn fs_list_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<Vec<CloudFsEntry>>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    let entries = sandbox
        .fs()
        .list(&query.path)
        .await?
        .into_iter()
        .map(cloud_fs_entry_from_sdk)
        .collect();
    Ok(Json(entries))
}

async fn fs_stat_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<CloudFsMetadata>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    Ok(Json(cloud_fs_metadata_from_sdk(
        sandbox.fs().stat(&query.path).await?,
    )))
}

async fn fs_mkdir_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloudFsPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    sandbox.fs().mkdir(&req.path).await?;
    Ok(Json(CloudMessageResponse {
        message: "directory created".into(),
    }))
}

async fn fs_remove_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsRemoveQuery>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    if query.recursive {
        sandbox.fs().remove_dir(&query.path).await?;
    } else {
        sandbox.fs().remove(&query.path).await?;
    }
    Ok(Json(CloudMessageResponse {
        message: "path removed".into(),
    }))
}

async fn fs_copy_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloudFsTwoPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    sandbox.fs().copy(&req.from, &req.to).await?;
    Ok(Json(CloudMessageResponse {
        message: "path copied".into(),
    }))
}

async fn fs_rename_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloudFsTwoPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    sandbox.fs().rename(&req.from, &req.to).await?;
    Ok(Json(CloudMessageResponse {
        message: "path renamed".into(),
    }))
}

async fn fs_exists_by_id(
    _auth: Auth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<CloudFsExistsResponse>, ApiError> {
    let sandbox = live_sandbox_by_id(&state, &id).await?;
    Ok(Json(CloudFsExistsResponse {
        exists: sandbox.fs().exists(&query.path).await?,
    }))
}

async fn create_volume(
    _auth: Auth,
    State(state): State<AppState>,
    Json(req): Json<VolumeConfig>,
) -> Result<Json<CloudVolume>, ApiError> {
    let handle = with_local_backend(&state, async {
        let volume = Volume::create(req).await?;
        Volume::get(volume.name()).await
    })
    .await?;
    Ok(Json(cloud_volume_from_handle(&handle)))
}

async fn list_volumes(
    _auth: Auth,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<CloudPaginated<CloudVolume>>, ApiError> {
    let mut handles = with_local_backend(&state, Volume::list()).await?;
    handles.sort_by(|a, b| a.name().cmp(b.name()));

    let mut data = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Some(cursor) = &query.cursor
            && handle.name() <= cursor.as_str()
        {
            continue;
        }
        data.push(cloud_volume_from_handle(&handle));
        if let Some(limit) = query.limit
            && data.len() >= limit as usize
        {
            break;
        }
    }

    let next_cursor = query
        .limit
        .and_then(|limit| {
            (data.len() >= limit as usize).then(|| data.last().map(|v| v.name.clone()))
        })
        .flatten();

    Ok(Json(CloudPaginated { data, next_cursor }))
}

async fn get_volume_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudVolume>, ApiError> {
    let handle = with_local_backend(&state, Volume::get(&name)).await?;
    Ok(Json(cloud_volume_from_handle(&handle)))
}

async fn remove_volume_by_name(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, Volume::remove(&name)).await?;
    Ok(Json(CloudMessageResponse {
        message: format!("volume {name} removed"),
    }))
}

async fn volume_fs_read(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Bytes, ApiError> {
    Ok(with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().read(&query.path).await
    })
    .await?)
}

async fn volume_fs_write(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsPathQuery>,
    body: Bytes,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().write(&query.path, body).await
    })
    .await?;
    Ok(Json(CloudMessageResponse {
        message: "file written".into(),
    }))
}

async fn volume_fs_list(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<Vec<CloudFsEntry>>, ApiError> {
    let entries = with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().list(&query.path).await
    })
    .await?
    .into_iter()
    .map(cloud_fs_entry_from_sdk)
    .collect();
    Ok(Json(entries))
}

async fn volume_fs_stat(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<CloudFsMetadata>, ApiError> {
    let metadata = with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().stat(&query.path).await
    })
    .await?;
    Ok(Json(cloud_fs_metadata_from_sdk(metadata)))
}

async fn volume_fs_mkdir(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CloudFsPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().mkdir(&req.path).await
    })
    .await?;
    Ok(Json(CloudMessageResponse {
        message: "directory created".into(),
    }))
}

async fn volume_fs_remove(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsRemoveQuery>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        if query.recursive {
            handle.fs().remove_dir(&query.path).await
        } else {
            handle.fs().remove(&query.path).await
        }
    })
    .await?;
    Ok(Json(CloudMessageResponse {
        message: "path removed".into(),
    }))
}

async fn volume_fs_copy(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CloudFsTwoPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().copy(&req.from, &req.to).await
    })
    .await?;
    Ok(Json(CloudMessageResponse {
        message: "path copied".into(),
    }))
}

async fn volume_fs_rename(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CloudFsTwoPathRequest>,
) -> Result<Json<CloudMessageResponse>, ApiError> {
    with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().rename(&req.from, &req.to).await
    })
    .await?;
    Ok(Json(CloudMessageResponse {
        message: "path renamed".into(),
    }))
}

async fn volume_fs_exists(
    _auth: Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<Json<CloudFsExistsResponse>, ApiError> {
    let exists = with_local_backend(&state, async {
        let handle = Volume::get(&name).await?;
        handle.fs().exists(&query.path).await
    })
    .await?;
    Ok(Json(CloudFsExistsResponse { exists }))
}

//--------------------------------------------------------------------------------------------------
// WebSocket exec
//--------------------------------------------------------------------------------------------------

async fn handle_exec_ws(state: AppState, name: String, socket: WebSocket) {
    if let Err(error) = handle_exec_ws_inner(state, &name, socket).await {
        error!(sandbox = %name, error = %error, "cloud exec websocket failed");
    }
}

async fn handle_exec_ws_inner(
    state: AppState,
    name: &str,
    socket: WebSocket,
) -> MicrosandboxResult<()> {
    let (mut writer, mut reader) = socket.split();
    let mut frame_buf = Vec::new();
    let Some(req_msg) = next_cbor_message(&mut reader, &mut frame_buf).await? else {
        return Err(MicrosandboxError::Runtime(
            "cloud exec websocket closed before ExecRequest".into(),
        ));
    };

    if req_msg.t != MessageType::ExecRequest {
        return Err(MicrosandboxError::Runtime(format!(
            "expected ExecRequest, got {:?}",
            req_msg.t
        )));
    }
    let req = req_msg.payload::<ExecRequest>()?;
    let backend: Arc<dyn Backend> = state.local.clone();
    let sandbox = microsandbox::with_backend(backend, async {
        let handle = Sandbox::get(name).await?;
        match handle.status_snapshot() {
            SandboxStatus::Running | SandboxStatus::Draining => handle.connect().await,
            _ => handle.start_detached().await,
        }
    })
    .await?;

    let ExecRequest {
        cmd,
        args,
        env,
        cwd,
        user,
        tty,
        rows: _,
        cols: _,
        rlimits,
    } = req;

    let mut handle = sandbox
        .exec_stream_with(cmd, |builder| {
            let mut builder = builder.args(args).tty(tty);
            if let Some(cwd) = cwd {
                builder = builder.cwd(cwd);
            }
            if let Some(user) = user {
                builder = builder.user(user);
            }
            for env in env {
                if let Some((key, value)) = env.split_once('=') {
                    builder = builder.env(key.to_string(), value.to_string());
                }
            }
            for rlimit in rlimits {
                if let Ok(resource) =
                    microsandbox::sandbox::RlimitResource::try_from(rlimit.resource.as_str())
                {
                    builder = builder.rlimit_range(resource, rlimit.soft, rlimit.hard);
                }
            }
            builder.stdin_pipe()
        })
        .await?;

    let mut stdin = handle.take_stdin();
    loop {
        tokio::select! {
            incoming = next_cbor_message(&mut reader, &mut frame_buf) => {
                match incoming? {
                    Some(message) => handle_exec_control_message(&mut stdin, &handle, message).await?,
                    None => {
                        if let Some(stdin) = stdin.take() {
                            let _ = stdin.close().await;
                        }
                        return Ok(());
                    }
                }
            }
            event = handle.recv() => {
                match event {
                    Some(microsandbox::ExecEvent::Started { pid }) => {
                        send_exec_message(&mut writer, MessageType::ExecStarted, &ExecStarted { pid }).await?;
                    }
                    Some(microsandbox::ExecEvent::Stdout(data)) => {
                        send_exec_message(&mut writer, MessageType::ExecStdout, &ExecStdout { data: data.to_vec() }).await?;
                    }
                    Some(microsandbox::ExecEvent::Stderr(data)) => {
                        send_exec_message(&mut writer, MessageType::ExecStderr, &ExecStderr { data: data.to_vec() }).await?;
                    }
                    Some(microsandbox::ExecEvent::Exited { code }) => {
                        send_exec_message(&mut writer, MessageType::ExecExited, &ExecExited { code }).await?;
                        let _ = writer.close().await;
                        return Ok(());
                    }
                    Some(microsandbox::ExecEvent::Failed(failed)) => {
                        send_exec_message(&mut writer, MessageType::ExecFailed, &failed).await?;
                        let _ = writer.close().await;
                        return Ok(());
                    }
                    Some(microsandbox::ExecEvent::StdinError(_)) => {}
                    None => {
                        let failed = ExecFailed {
                            kind: microsandbox_protocol::exec::ExecFailureKind::Other,
                            errno: None,
                            errno_name: None,
                            message: "local exec stream ended without exit event".into(),
                            stage: Some("cloud-adapter".into()),
                        };
                        send_exec_message(&mut writer, MessageType::ExecFailed, &failed).await?;
                        let _ = writer.close().await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn next_cbor_message(
    reader: &mut futures::stream::SplitStream<WebSocket>,
    frame_buf: &mut Vec<u8>,
) -> MicrosandboxResult<Option<Message>> {
    loop {
        if let Some(message) = codec::try_decode_from_buf(frame_buf)? {
            return Ok(Some(message));
        }

        let Some(frame) = reader.next().await else {
            return Ok(None);
        };
        match frame {
            Ok(WsMessage::Binary(bytes)) => frame_buf.extend_from_slice(&bytes),
            Ok(WsMessage::Close(_)) => return Ok(None),
            Ok(_) => {}
            Err(error) => {
                return Err(MicrosandboxError::Runtime(format!(
                    "websocket read failed: {error}"
                )));
            }
        }
    }
}

async fn handle_exec_control_message(
    stdin: &mut Option<microsandbox::sandbox::exec::ExecSink>,
    handle: &microsandbox::ExecHandle,
    message: Message,
) -> MicrosandboxResult<()> {
    match message.t {
        MessageType::ExecStdin => {
            let payload = message.payload::<ExecStdin>()?;
            if payload.data.is_empty() {
                if let Some(stdin) = stdin.take() {
                    stdin.close().await?;
                }
            } else if let Some(stdin) = stdin {
                stdin.write(payload.data).await?;
            }
        }
        MessageType::ExecResize => {
            let payload = message.payload::<microsandbox_protocol::exec::ExecResize>()?;
            handle.resize(payload.rows, payload.cols).await?;
        }
        MessageType::ExecSignal => {
            let payload = message.payload::<microsandbox_protocol::exec::ExecSignal>()?;
            handle.signal(payload.signal).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn send_exec_message<T: Serialize>(
    writer: &mut futures::stream::SplitSink<WebSocket, WsMessage>,
    t: MessageType,
    payload: &T,
) -> MicrosandboxResult<()> {
    let message = Message::with_payload(t, 1, payload)?;
    let mut buf = Vec::new();
    codec::encode_to_buf(&message, &mut buf)?;
    writer
        .send(WsMessage::Binary(Bytes::from(buf)))
        .await
        .map_err(|e| MicrosandboxError::Runtime(format!("websocket write failed: {e}")))
}

//--------------------------------------------------------------------------------------------------
// Auth
//--------------------------------------------------------------------------------------------------

impl<S> FromRequestParts<S> for Auth
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let header = bearer_token(&parts.headers).ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "missing Authorization: Bearer token".into(),
        })?;

        if header != state.api_key.as_ref() {
            return Err(ApiError {
                status: StatusCode::UNAUTHORIZED,
                code: "unauthorized",
                message: "invalid API key".into(),
            });
        }

        Ok(Auth)
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value.strip_prefix("Bearer ")
}

//--------------------------------------------------------------------------------------------------
// Conversion
//--------------------------------------------------------------------------------------------------

fn sandbox_config_from_cloud_request(
    req: &CloudCreateSandboxRequest,
) -> Result<SandboxConfig, ApiError> {
    microsandbox::validate_sandbox_name(&req.name)?;
    let spec = SandboxSpec {
        name: req.name.clone(),
        image: microsandbox::sandbox::RootfsSource::Oci(microsandbox::sandbox::OciRootfsSource {
            reference: req.image.clone(),
            upper_size_mib: None,
        }),
        resources: SandboxResources {
            cpus: req.vcpus,
            memory_mib: req.memory_mib,
        },
        runtime: SandboxRuntimeOptions {
            workdir: req.workdir.clone(),
            shell: req.shell.clone(),
            scripts: req.scripts.clone().into_iter().collect(),
            entrypoint: req.entrypoint.clone(),
            hostname: req.hostname.clone(),
            user: req.user.clone(),
            log_level: req
                .log_level
                .as_deref()
                .and_then(|level| level.parse().ok()),
            ..Default::default()
        },
        env: req
            .env
            .iter()
            .map(|(key, value)| EnvVar::new(key.clone(), value.clone()))
            .collect(),
        lifecycle: SandboxPolicy {
            ephemeral: req.ephemeral,
            max_duration_secs: req.max_duration_secs,
            idle_timeout_secs: req.idle_timeout_secs,
        },
        ..Default::default()
    };

    let mut config = SandboxConfig::default();
    config.spec = spec;
    Ok(config)
}

fn cloud_request_from_handle(
    handle: &microsandbox::sandbox::SandboxHandle,
) -> Result<CloudCreateSandboxRequest, ApiError> {
    let config = handle.config()?;
    cloud_request_from_config(&config)
}

fn cloud_request_from_config(
    config: &SandboxConfig,
) -> Result<CloudCreateSandboxRequest, ApiError> {
    let image = match &config.spec.image {
        microsandbox::sandbox::RootfsSource::Oci(image) => image.reference.clone(),
        _ => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "unsupported_rootfs",
                message: "cloud adapter only exposes OCI-backed sandboxes through the cloud API"
                    .into(),
            });
        }
    };

    Ok(CloudCreateSandboxRequest {
        name: config.spec.name.clone(),
        image,
        vcpus: config.spec.resources.cpus,
        memory_mib: config.spec.resources.memory_mib,
        env: config
            .spec
            .env
            .clone()
            .into_iter()
            .map(Into::into)
            .collect(),
        ephemeral: config.spec.lifecycle.ephemeral,
        workdir: config.spec.runtime.workdir.clone(),
        shell: config.spec.runtime.shell.clone(),
        entrypoint: config.spec.runtime.entrypoint.clone(),
        hostname: config.spec.runtime.hostname.clone(),
        user: config.spec.runtime.user.clone(),
        log_level: config
            .spec
            .runtime
            .log_level
            .map(|level| level.as_str().to_string()),
        scripts: config.spec.runtime.scripts.clone().into_iter().collect(),
        max_duration_secs: config.spec.lifecycle.max_duration_secs,
        idle_timeout_secs: config.spec.lifecycle.idle_timeout_secs,
    })
}

async fn cloud_sandbox_from_live(
    _sandbox: &Sandbox,
    req: &CloudCreateSandboxRequest,
) -> Result<CloudSandbox, ApiError> {
    let handle = Sandbox::get(&req.name).await?;
    Ok(cloud_sandbox_from_handle(&handle, req.clone()))
}

async fn lifecycle_response_after_operation(
    state: &AppState,
    name: &str,
    original: &microsandbox::sandbox::SandboxHandle,
    req: CloudCreateSandboxRequest,
    missing_status: CloudSandboxStatus,
) -> Result<CloudSandbox, ApiError> {
    match with_local_backend(state, Sandbox::get(name)).await {
        Ok(refreshed) => Ok(cloud_sandbox_from_handle(&refreshed, req)),
        Err(err)
            if err.status == StatusCode::NOT_FOUND
                && err.code == "sandbox_not_found"
                && req.ephemeral =>
        {
            Ok(cloud_sandbox_terminal_from_handle(
                original,
                req,
                missing_status,
            ))
        }
        Err(err) => Err(err),
    }
}

fn cloud_sandbox_from_handle(
    handle: &microsandbox::sandbox::SandboxHandle,
    req: CloudCreateSandboxRequest,
) -> CloudSandbox {
    CloudSandbox {
        id: handle.name().to_string(),
        org_id: ORG_ID.into(),
        name: handle.name().to_string(),
        status: cloud_status_from_local(handle.status_snapshot()),
        config: req,
        ephemeral: handle
            .config()
            .ok()
            .map(|c| c.spec.lifecycle.ephemeral)
            .unwrap_or(true),
        created_at: handle.created_at().unwrap_or_else(chrono::Utc::now),
        started_at: running_started_at(handle),
        stopped_at: stopped_at(handle),
        last_error: handle.last_error_snapshot(),
    }
}

fn cloud_sandbox_terminal_from_handle(
    handle: &microsandbox::sandbox::SandboxHandle,
    req: CloudCreateSandboxRequest,
    status: CloudSandboxStatus,
) -> CloudSandbox {
    let mut sandbox = cloud_sandbox_from_handle(handle, req);
    sandbox.status = status;
    sandbox.stopped_at = Some(chrono::Utc::now());
    sandbox
}

fn cloud_volume_from_handle(handle: &VolumeHandle) -> CloudVolume {
    CloudVolume {
        id: handle.name().to_string(),
        org_id: ORG_ID.into(),
        name: handle.name().to_string(),
        kind: handle.kind(),
        quota_mib: handle.quota_mib(),
        used_bytes: handle.used_bytes(),
        capacity_bytes: handle.capacity_bytes(),
        disk_format: handle.disk_format().map(ToOwned::to_owned),
        disk_fstype: handle.disk_fstype().map(ToOwned::to_owned),
        labels: handle.labels().to_vec(),
        created_at: handle.created_at(),
    }
}

fn cloud_status_from_local(status: SandboxStatus) -> CloudSandboxStatus {
    match status {
        SandboxStatus::Created => CloudSandboxStatus::Created,
        SandboxStatus::Starting => CloudSandboxStatus::Starting,
        SandboxStatus::Running => CloudSandboxStatus::Running,
        SandboxStatus::Draining => CloudSandboxStatus::Stopping,
        SandboxStatus::Paused => CloudSandboxStatus::Running,
        SandboxStatus::Stopped => CloudSandboxStatus::Stopped,
        SandboxStatus::Crashed => CloudSandboxStatus::Failed,
    }
}

fn running_started_at(
    handle: &microsandbox::sandbox::SandboxHandle,
) -> Option<chrono::DateTime<chrono::Utc>> {
    matches!(
        handle.status_snapshot(),
        SandboxStatus::Running | SandboxStatus::Draining
    )
    .then(|| handle.updated_at().or_else(|| handle.created_at()))
    .flatten()
}

fn stopped_at(
    handle: &microsandbox::sandbox::SandboxHandle,
) -> Option<chrono::DateTime<chrono::Utc>> {
    matches!(
        handle.status_snapshot(),
        SandboxStatus::Stopped | SandboxStatus::Crashed
    )
    .then(|| handle.updated_at())
    .flatten()
}

fn sandbox_name_from_cloud_id(id: &str) -> String {
    id.to_string()
}

fn parse_log_sources(
    sources: Option<&str>,
) -> Result<Vec<microsandbox::logs::LogSource>, ApiError> {
    let Some(sources) = sources else {
        return Ok(Vec::new());
    };
    sources
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|source| match source {
            "stdout" => Ok(microsandbox::logs::LogSource::Stdout),
            "stderr" => Ok(microsandbox::logs::LogSource::Stderr),
            "output" => Ok(microsandbox::logs::LogSource::Output),
            "system" => Ok(microsandbox::logs::LogSource::System),
            other => Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_log_source",
                message: format!("unsupported log source {other:?}"),
            }),
        })
        .collect()
}

fn log_source_name(source: microsandbox::logs::LogSource) -> &'static str {
    match source {
        microsandbox::logs::LogSource::Stdout => "stdout",
        microsandbox::logs::LogSource::Stderr => "stderr",
        microsandbox::logs::LogSource::Output => "output",
        microsandbox::logs::LogSource::System => "system",
    }
}

async fn live_sandbox_by_id(state: &AppState, id: &str) -> Result<Sandbox, ApiError> {
    let name = sandbox_name_from_cloud_id(id);
    let backend: Arc<dyn Backend> = state.local.clone();
    Ok(microsandbox::with_backend(backend, async {
        let handle = Sandbox::get(&name).await?;
        match handle.status_snapshot() {
            SandboxStatus::Running | SandboxStatus::Draining => handle.connect().await,
            status => Err(MicrosandboxError::Custom(format!(
                "sandbox {name:?} is not running (status: {status:?})"
            ))),
        }
    })
    .await?)
}

fn cloud_metrics_from_sdk(metrics: SandboxMetrics) -> CloudSandboxMetrics {
    CloudSandboxMetrics {
        cpu_percent: metrics.cpu_percent,
        vcpu_time_ns: metrics.vcpu_time_ns,
        memory_bytes: metrics.memory_bytes,
        memory_available_bytes: metrics.memory_available_bytes,
        memory_host_resident_bytes: metrics.memory_host_resident_bytes,
        memory_limit_bytes: metrics.memory_limit_bytes,
        disk_read_bytes: metrics.disk_read_bytes,
        disk_write_bytes: metrics.disk_write_bytes,
        net_rx_bytes: metrics.net_rx_bytes,
        net_tx_bytes: metrics.net_tx_bytes,
        upper_used_bytes: metrics.upper_used_bytes,
        upper_free_bytes: metrics.upper_free_bytes,
        upper_host_allocated_bytes: metrics.upper_host_allocated_bytes,
        uptime_ms: metrics.uptime.as_millis().min(u128::from(u64::MAX)) as u64,
        timestamp: metrics.timestamp,
    }
}

fn cloud_fs_entry_from_sdk(entry: FsEntry) -> CloudFsEntry {
    CloudFsEntry {
        path: entry.path,
        kind: cloud_fs_kind_from_sdk(entry.kind),
        size: entry.size,
        mode: entry.mode,
        modified: entry.modified,
    }
}

fn cloud_fs_metadata_from_sdk(metadata: FsMetadata) -> CloudFsMetadata {
    CloudFsMetadata {
        kind: cloud_fs_kind_from_sdk(metadata.kind),
        size: metadata.size,
        mode: metadata.mode,
        readonly: metadata.readonly,
        modified: metadata.modified,
        created: metadata.created,
    }
}

fn cloud_fs_kind_from_sdk(kind: FsEntryKind) -> CloudFsEntryKind {
    match kind {
        FsEntryKind::File => CloudFsEntryKind::File,
        FsEntryKind::Directory => CloudFsEntryKind::Directory,
        FsEntryKind::Symlink => CloudFsEntryKind::Symlink,
        FsEntryKind::Other => CloudFsEntryKind::Other,
    }
}

async fn with_local_backend<T>(
    state: &AppState,
    future: impl std::future::Future<Output = MicrosandboxResult<T>>,
) -> Result<T, ApiError> {
    let backend: Arc<dyn Backend> = state.local.clone();
    microsandbox::with_backend(backend, future)
        .await
        .map_err(Into::into)
}

//--------------------------------------------------------------------------------------------------
// Errors
//--------------------------------------------------------------------------------------------------

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorEnvelope {
            error: ErrorDetails {
                code: self.code,
                message: self.message,
            },
        });
        (self.status, body).into_response()
    }
}

impl From<MicrosandboxError> for ApiError {
    fn from(value: MicrosandboxError) -> Self {
        let (status, code) = match &value {
            MicrosandboxError::SandboxNotFound(_) => (StatusCode::NOT_FOUND, "sandbox_not_found"),
            MicrosandboxError::SandboxAlreadyExists(_) => {
                (StatusCode::CONFLICT, "sandbox_already_exists")
            }
            MicrosandboxError::SandboxStillRunning(_) => {
                (StatusCode::CONFLICT, "sandbox_still_running")
            }
            MicrosandboxError::VolumeNotFound(_) => (StatusCode::NOT_FOUND, "volume_not_found"),
            MicrosandboxError::VolumeAlreadyExists(_) => {
                (StatusCode::CONFLICT, "volume_already_exists")
            }
            MicrosandboxError::InvalidConfig(_) => (StatusCode::BAD_REQUEST, "invalid_config"),
            MicrosandboxError::Unsupported { .. } => (StatusCode::BAD_REQUEST, "unsupported"),
            MicrosandboxError::ExecTimeout(_) => (StatusCode::REQUEST_TIMEOUT, "exec_timeout"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };

        Self {
            status,
            code,
            message: value.to_string(),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "json",
            message: value.to_string(),
        }
    }
}

impl From<microsandbox_protocol::ProtocolError> for ApiError {
    fn from(value: microsandbox_protocol::ProtocolError) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "protocol",
            message: value.to_string(),
        }
    }
}
