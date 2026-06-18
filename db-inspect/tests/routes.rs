use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

use db_inspect::db::AppState;
use db_inspect::routes::create_router;

fn fixture_path() -> String {
    format!("{}/tests/fixtures/voicebot.db", env!("CARGO_MANIFEST_DIR"))
}

async fn app() -> impl tower::Service<
    Request<Body>,
    Response = axum::http::Response<Body>,
    Error = std::convert::Infallible,
> {
    let fixture = fixture_path();
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("voicebot.db");
    std::fs::copy(&fixture, &db_path).expect("failed to copy fixture db");

    // Leak the temp dir so the database file remains accessible for the
    // lifetime of the test. This is acceptable in test code.
    let _ = Box::leak(Box::new(temp_dir));

    let state = Arc::new(
        AppState::new(db_path.to_str().unwrap())
            .await
            .expect("failed to create app state"),
    );
    create_router(state)
}

#[tokio::test]
async fn test_home() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_sessions_list() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/sessions").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_session_detail() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/sessions/550e8400-e29b-41d4-a716-446655440000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_session_detail_not_found() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/sessions/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_messages_list() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/messages").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_profile() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/profile").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_memories() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/memories").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_history() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/history").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_dream_state() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/dream-state").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_system_prompts() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/system-prompts").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_create_system_prompt() {
    let app = app().await;
    let body = "session_id=550e8400-e29b-41d4-a716-446655440001&content=New+test+prompt&active=on";
    let resp = app
        .oneshot(
            Request::post("/system-prompts")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/system-prompts");
}

#[tokio::test]
async fn test_delete_system_prompt() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/system-prompts/1/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/system-prompts");
}

#[tokio::test]
async fn test_activate_system_prompt() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/system-prompts/1/activate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/system-prompts");
}

#[tokio::test]
async fn test_search_with_query() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/search?q=Rust").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_search_without_query() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/search").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_confirmation() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/sessions/550e8400-e29b-41d4-a716-446655440000/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_confirmation_not_found() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/sessions/00000000-0000-0000-0000-000000000000/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_message_delete_confirmation() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/messages/1/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_message_delete_confirmation_not_found() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/messages/999999/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_message_delete_post() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/messages/1/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").unwrap().as_bytes(),
        b"/messages?deleted=1"
    );
}
