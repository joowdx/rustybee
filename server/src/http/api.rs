//! Public API `/api/v1` (spec 007).

use super::auth::ApiKey;
use crate::app::{App, Draft, VERSION};
use crate::domain::batch_status;
use crate::domain::lifecycle::State as MsgState;
use crate::error::ApiError;
use crate::store::{devices, messages};
use crate::util::{b64url, b64url_decode, parse_rfc3339};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/messages", post(send_messages).get(list_messages))
        .route("/api/v1/messages/{id}", get(get_message))
        .route("/api/v1/batches", post(send_batch))
        .route("/api/v1/batches/{id}", get(get_batch))
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/{*rest}", any(not_found))
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "version": VERSION}))
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

#[derive(Deserialize)]
struct SendReq {
    to: Vec<String>,
    body: String,
    device_id: Option<String>,
    sim: Option<i64>,
}

#[derive(Deserialize)]
struct BatchItem {
    to: String,
    body: String,
}

#[derive(Deserialize)]
struct BatchReq {
    items: Vec<BatchItem>,
    device_id: Option<String>,
    sim: Option<i64>,
}

fn accepted(batch: String, rows: Vec<messages::MessageRow>) -> (StatusCode, Json<Value>) {
    let msgs: Vec<Value> = rows.iter().map(|r| r.to_json()).collect();
    (
        StatusCode::ACCEPTED,
        Json(json!({"batch_id": batch, "messages": msgs})),
    )
}

async fn send_messages(
    _: ApiKey,
    State(app): State<App>,
    req: Result<Json<SendReq>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Json(req) = req?;
    let drafts = req
        .to
        .into_iter()
        .enumerate()
        .map(|(i, to)| Draft {
            label: format!("to[{i}]"),
            to,
            body: req.body.clone(),
        })
        .collect();
    let (batch, rows) = app.queue(drafts, req.device_id, req.sim).await?;
    Ok(accepted(batch, rows))
}

async fn send_batch(
    _: ApiKey,
    State(app): State<App>,
    req: Result<Json<BatchReq>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Json(req) = req?;
    let drafts = req
        .items
        .into_iter()
        .enumerate()
        .map(|(i, it)| Draft {
            label: format!("items[{i}].to"),
            to: it.to,
            body: it.body,
        })
        .collect();
    let (batch, rows) = app.queue(drafts, req.device_id, req.sim).await?;
    Ok(accepted(batch, rows))
}

async fn get_batch(
    _: ApiKey,
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let pid = id.clone();
    let (_, device, total, counts) = app
        .store
        .read(move |c| messages::batch_counts(c, &pid))
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({
        "id": id,
        "device_id": device,
        "total": total,
        "counts": counts,
        "status": batch_status(&counts),
    })))
}

#[derive(Deserialize, Default)]
pub struct ListParams {
    pub direction: Option<String>,
    pub state: Option<String>,
    pub device_id: Option<String>,
    pub peer: Option<String>,
    pub batch_id: Option<String>,
    pub q: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<String>,
    pub cursor: Option<String>,
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn encode_cursor(id: i64) -> String {
    b64url(id.to_string().as_bytes())
}

/// Validate list parameters into a store query.
pub fn parse_list(p: ListParams, default_limit: i64) -> Result<messages::Query, ApiError> {
    let direction = nonempty(p.direction);
    if direction
        .as_deref()
        .is_some_and(|d| d != "in" && d != "out")
    {
        return Err(ApiError::bad_request("direction must be `in` or `out`"));
    }
    let state = nonempty(p.state);
    if let Some(s) = &state
        && s.parse::<MsgState>().is_err()
    {
        return Err(ApiError::bad_request(format!("unknown state {s:?}")));
    }
    let time = |v: Option<String>, name: &str| -> Result<Option<i64>, ApiError> {
        nonempty(v)
            .map(|s| {
                parse_rfc3339(&s)
                    .ok_or_else(|| ApiError::bad_request(format!("{name}: not an RFC 3339 time")))
            })
            .transpose()
    };
    let limit = match nonempty(p.limit) {
        None => default_limit,
        Some(l) => match l.parse::<i64>() {
            Ok(n) if (1..=200).contains(&n) => n,
            _ => return Err(ApiError::bad_request("limit must be 1..=200")),
        },
    };
    let before_id = match nonempty(p.cursor) {
        None => None,
        Some(c) => Some(
            b64url_decode(&c)
                .and_then(|b| String::from_utf8(b).ok())
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| ApiError::bad_request("invalid cursor"))?,
        ),
    };
    Ok(messages::Query {
        direction,
        state,
        device_id: nonempty(p.device_id),
        peer: nonempty(p.peer),
        batch_id: nonempty(p.batch_id),
        q: nonempty(p.q),
        since: time(p.since, "since")?,
        until: time(p.until, "until")?,
        before_id,
        limit,
        include_internal: false,
    })
}

async fn list_messages(
    _: ApiKey,
    State(app): State<App>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(params) = params?;
    let q = parse_list(params, 50)?;
    let (rows, next) = app.store.read(move |c| messages::list(c, &q)).await?;
    let items: Vec<Value> = rows.iter().map(|r| r.to_json()).collect();
    Ok(Json(
        json!({"items": items, "next_cursor": next.map(encode_cursor)}),
    ))
}

async fn get_message(
    _: ApiKey,
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let m = app
        .store
        .read(move |c| messages::by_public(c, &id))
        .await?
        .filter(|m| !m.internal)
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(m.to_json()))
}

async fn list_devices(_: ApiKey, State(app): State<App>) -> Result<Json<Value>, ApiError> {
    let list = app.store.read(devices::list).await?;
    Ok(Json(Value::Array(
        list.iter().map(|d| d.to_json()).collect(),
    )))
}
