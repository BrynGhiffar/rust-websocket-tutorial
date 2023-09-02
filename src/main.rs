use actix_web::{HttpServer, App, web, Responder};

async fn healthcheck() -> impl Responder {
    "rust-websocket-tutorial is live"
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let port = 5000;

    log::info!("rust-websocket-tutorial will be running on http://0.0.0.0:{port}");
    let http_server = HttpServer::new(move || {
        App::new()
            .service(web::resource("/healthcheck").route(web::get().to(healthcheck)))
    })
    .workers(2)
    .bind(("0.0.0.0", port))?
    .run();

    http_server.await?;

    Ok(())
}
