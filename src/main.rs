mod analyze;
mod assets;
mod config;
mod db;
mod dictionary;
mod error;
mod fallback;
mod opus;
mod routes;
mod script;
mod serialize;
mod spa;
mod state;
mod synth;
mod voicepeak;
mod wav;

use config::Config;
use state::AppState;
use std::sync::{Arc, Mutex};
use synth::SynthQueue;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let conn = db::open(&config.db_path);
    let analyzer = analyze::create_analyzer_from_env();
    // 時事ネタ声かけ専用の CLI backend (例: WORK_NEWS_CMD="claude -p --allowed-tools WebSearch")。
    // 未設定なら /work/talk の news も通常 analyzer にフォールバックする。
    let news_analyzer = std::env::var("WORK_NEWS_CMD")
        .ok()
        .map(|cmd| cmd.trim().to_string())
        .filter(|cmd| !cmd.is_empty())
        .map(|cmd| Arc::new(analyze::Backend::Cli(analyze::CliAnalyzer::new(cmd))));
    // 英単語→カタカナのフォールバック辞書を起動時に 1 回だけロード (DL 失敗時は無効化)。
    let fallback =
        Arc::new(fallback::load_or_download(&config.bep_dict_path, &config.bep_dict_url).await);
    if fallback.is_empty() {
        tracing::info!("fallback dict disabled (not loaded)");
    } else {
        tracing::info!("fallback dict loaded");
    }
    // 作業タブの立ち絵素材。無ければバックグラウンドで公式サイトから DL して展開する
    // (約150MB — 起動はブロックしない。完了までクライアントは立ち絵なしで動く)。
    tokio::spawn(assets::ensure_illust(
        config.assets_dir.clone(),
        config.moca_illust_url.clone(),
    ));

    // 通知 broadcast。購読者ゼロでも send は Err になるだけで捨てられる (fire-and-forget)。
    let (notify, _) = tokio::sync::broadcast::channel::<String>(64);

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        config: config.clone(),
        synth: Arc::new(SynthQueue::new()),
        analyzer: Arc::new(analyzer),
        news_analyzer,
        fallback,
        notify,
    };

    let app = routes::build_router(state);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");
    tracing::info!("moca-server running on http://localhost:{}/", config.port);
    axum::serve(listener, app).await.expect("Server error");
}
