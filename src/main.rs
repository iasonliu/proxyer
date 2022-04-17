use anyhow::*;
use clap::Parser;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Server};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[clap(name = "proxyer", about = "A http proxyer server")]
struct Config {
    #[clap(short = 'l', long = "log", default_value = "debug")]
    log_level: String,
    #[clap(short = 'a', long = "addr", default_value = "::1")]
    addr: String,
    #[clap(short = 'p', long = "port", default_value = "3000")]
    port: u16,
}

fn mutate_request(req: &mut Request<Body>) -> Result<()> {
    for key in [
        "content-length",
        "transfer-encoding",
        "accept-encoding",
        "content-encoding",
    ] {
        req.headers_mut().remove(key);
    }

    let uri = req.uri();
    let uri_string = match uri.query() {
        None => format!("https://httpbin.org{}", uri.path()),
        Some(query) => format!("https://www.httpbin.org{}?{}", uri.path(), query),
    };
    *req.uri_mut() = uri_string
        .parse()
        .context("Parsing URI in mutate_request")?;
    Ok(())
}

async fn shutdown_signal() {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

#[derive(Debug)]
struct Stats {
    proxied: usize,
}
#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var(
            "RUST_LOG",
            format!("{},hyper=info,mio=info", config.log_level),
        )
    }
    tracing_subscriber::fmt::init();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_only()
        .enable_http1()
        .build();

    let client: Client<_, hyper::Body> = Client::builder().build(https);
    let client = Arc::new(client);
    let stats: Arc<RwLock<Stats>> = Arc::new(RwLock::new(Stats { proxied: 0 }));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    let make_svc = make_service_fn(move |_| {
        let client = Arc::clone(&client);
        let stats = Arc::clone(&stats);
        async move {
            Ok::<_>(service_fn(move |mut req| {
                let stats = Arc::clone(&stats);
                let client = Arc::clone(&client);
                async move {
                    if req.uri().path() == "/status" {
                        let stats: &Stats = &*stats.read().unwrap();
                        let body: Body = format!("{:?}", stats).into();
                        Ok(Response::new(body))
                    } else {
                        println!("Proxied: {}", req.uri().path());
                        stats.write().unwrap().proxied += 1;
                        let client = Arc::clone(&client);
                        mutate_request(&mut req)?;
                        client.request(req).await.context("proxy request")
                    }
                }
            }))
        }
    });
    info!("listening on http://{}", addr);
    let server = Server::bind(&addr).serve(make_svc);
    // And now add a graceful shutdown signal...
    let graceful = server.with_graceful_shutdown(shutdown_signal());

    if let Err(e) = graceful.await {
        error!("Server error: {}", e)
    }

    Ok(())
}
