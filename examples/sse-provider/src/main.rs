use actix_web::{web, App, HttpResponse, HttpServer};
use fakeit::{datetime, internet};
use log::*;
use rand::Rng;
use uuid::Uuid;

async fn get_events() -> HttpResponse {
    debug!("GET request for SSE events");

    let mut rng = rand::thread_rng();
    let event_id: u64 = rng.gen_range(1..10000);
    let count_value: i32 = rng.gen_range(0..100);
    let date = datetime::date_datetime();
    let user_id = Uuid::new_v4();

    let sse_data = format!(
        "id:{}\ndata:simple text\n\
         retry:3000\nevent:count\ndata:{}\n\
         event:time\ndata:{}\n\
         id:{}\nevent:user\ndata:{}\n",
        event_id, count_value, date.format("%Y-%m-%d"), event_id + 1000, user_id
    );

    HttpResponse::Ok()
        .content_type("text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(sse_data)
}

async fn get_events_with_headers() -> HttpResponse {
    debug!("GET request for SSE events with custom headers");

    let mut rng = rand::thread_rng();
    let last_event_id: u64 = rng.gen_range(1..1000);

    let sse_data = format!(
        "id:{}\nevent:heartbeat\ndata:{{\"status\":\"ok\"}}\n\
         id:{}\ndata:plain message\n",
        last_event_id,
        last_event_id + 1
    );

    HttpResponse::Ok()
        .content_type("text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(sse_data)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let _ = simple_log::quick();
    info!("Starting SSE provider on 127.0.0.1:8080");

    HttpServer::new(|| {
        App::new()
            .route("/events", web::get().to(get_events))
            .route(
                "/events/with-headers",
                web::get().to(get_events_with_headers),
            )
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
