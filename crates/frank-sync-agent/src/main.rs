//! frank-sync-agent 服务端入口。
//!
//! 起 axum HTTP server 监听 `0.0.0.0:3000` (容器内); 由 Caddy 反代 `https://tx:8318/memory/*`
//! `/orchestrator/*` 路由到这里。
//!
//! 环境变量:
//! - `FRANK_BIND_ADDR`        默认 `0.0.0.0:3000`
//! - `FRANK_QDRANT_URL`       默认 `http://qdrant:6334`
//! - `FRANK_COLLECTION`       默认 `frank_memories_v1`
//! - `OPENAI_API_KEY`         (必填) OpenAI embedder
//! - `ANTHROPIC_API_KEY`      (必填) Claude fact extractor
//! - `RUST_LOG`               日志级别, 默认 info

use std::env;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

mod local_embedder;
mod mock;
mod routes;
mod state;
mod tenant;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let bind: SocketAddr = env::var("FRANK_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse()
        .context("parse FRANK_BIND_ADDR")?;

    let app_state = state::AppState::from_env()
        .await
        .context("init AppState from env")?;

    // v0.12.0: 启动后台 retention worker (每小时扫一次到点的 tenant, 真删 qdrant + sqlite)
    routes::spawn_retention_worker(app_state.clone());

    let app = routes::router(app_state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;

    tracing::info!(%bind, "frank-sync-agent listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    tracing::info!("frank-sync-agent shutdown complete");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
