#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate prometheus;
#[macro_use]
extern crate prost_derive;

extern crate bytes;
#[macro_use]
extern crate futures;
extern crate http;
extern crate hyper;
extern crate nix;
extern crate prost;
extern crate rustracing;
extern crate rustracing_jaeger;
extern crate tokio_core;
extern crate tokio_signal;
extern crate tower_grpc;
extern crate tower_h2;
extern crate tower_http;

use futures::future::{self, Executor, FutureResult};
use futures::{Future, Stream};
use hyper::server::{Http, Request, Response, Service};
use prometheus::{Counter, Encoder, Gauge, TextEncoder};
use rustracing::sampler::AllSampler;
use rustracing::span::Span;
use rustracing_jaeger::Tracer;
use rustracing_jaeger::reporter::JaegerCompactReporter;
use rustracing_jaeger::span::SpanContextState;
use std::cell::Cell;
use std::io;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_signal::unix::{Signal, SIGQUIT};
use tower_grpc::Request as GrpcRequest;
use tower_h2::client::Connection;

lazy_static! {
    static ref SIGQUIT_COUNTER: Counter = register_counter!(
        opts!(
            "exp_sigquit_total",
            "Total number of signals received."
        )
    ).unwrap();

    static ref RKT_IMAGES_GAUGE: Gauge = register_gauge!(
        opts!(
            "rkt_images_total",
            "Total number of images in rkt store."
        )
    ).unwrap();
}

pub mod rkt {
    include!(concat!(env!("OUT_DIR"), "/v1alpha.rs"));
}

struct Metrics;
impl Service for Metrics {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = FutureResult<Response, hyper::Error>;

    fn call(&self, req: Request) -> Self::Future {
        use hyper::header::ContentType;
        use hyper::mime::Mime;
        future::ok(match (req.method(), req.path()) {
            (&hyper::Get, "/metrics") => {
                let encoder = TextEncoder::new();
                let mime = encoder.format_type().parse::<Mime>().unwrap();
                let mut buffer = vec![];
                encoder.encode(&prometheus::gather(), &mut buffer).unwrap();
                Response::new()
                    .with_header(ContentType(mime))
                    .with_body(buffer)
            }
            _ => Response::new().with_status(hyper::StatusCode::NotFound),
        })
    }
}

pub fn main() {
    run().unwrap();
}

type SpanCell = Cell<Option<Span<SpanContextState>>>;
task_local! { static TRACE: SpanCell = Cell::new(None) }

pub fn run() -> Result<(), failure::Error> {
    // Initialization.
    SIGQUIT_COUNTER.inc_by(0.0)?;
    RKT_IMAGES_GAUGE.set(0.0);
    let (tracer, span_rx) = Tracer::new(AllSampler);
    let reporter = JaegerCompactReporter::new("exp-rkt-grpc")?;
    let mut core = Core::new()?;

    // Background thread for reporting traces to Jaeger,
    // see https://github.com/sile/rustracing_jaeger/issues/4
    std::thread::spawn(move || {
        while let Ok(span) = span_rx.recv() {
            if let Err(e) = reporter.report(&[span][..]) {
                println!("{:?}", e);
            }
        }
    });

    // A stream of SIGQUIT events (ctrl-\).
    let signals = Signal::new(SIGQUIT, &core.handle())
        .flatten_stream()
        .inspect(move |_| TRACE.with(|t| t.set(Some(tracer.span("rkt-count-images").start()))))
        .inspect(|_| SIGQUIT_COUNTER.inc())
        .inspect(move |_| {
            TRACE.with(|t| {
                let mut span = t.take().unwrap();
                span.log(|log| {
                    log.std().event("signal counter");
                });
                t.set(Some(span));
            })
        });

    // A stream of GRPC requests to rkt api-service.
    let addr = "127.0.0.1:15441".parse()?;
    let uri: http::Uri = "http://localhost:15441".parse()?;
    let grpc_request = signals
        .and_then({
            let h = core.handle();
            move |_| TcpStream::connect(&addr, &h)
        })
        .inspect(move |_| {
            TRACE.with(|t| {
                let mut span = t.take().unwrap();
                span.log(|log| {
                    log.std().event("TCP socket");
                });
                t.set(Some(span));
            })
        })
        .and_then({
            let h = core.handle();
            move |socket| {
                Connection::handshake(socket, h.clone())
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed HTTP/2.0 handshake"))
            }
        })
        .inspect(move |_| {
            TRACE.with(|t| {
                let mut span = t.take().unwrap();
                span.log(|log| {
                    log.std().event("HTTP/2.0 connection");
                });
                t.set(Some(span));
            })
        })
        .map(move |conn| {
            let conn = tower_http::add_origin::Builder::new()
                .uri(uri.clone())
                .build(conn)
                .unwrap();
            rkt::client::PublicApi::new(conn)
        })
        .inspect(move |_| {
            TRACE.with(|t| {
                let mut span = t.take().unwrap();
                span.log(|log| {
                    log.std().event("gRPC connection");
                });
                t.set(Some(span));
            })
        })
        .and_then(|mut client| {
            use rkt::ListImagesRequest;
            client
                .list_images(GrpcRequest::new(ListImagesRequest {
                    detail: false,
                    filters: vec![],
                }))
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!("gRPC request failed; err={:?}", e),
                    )
                })
                .inspect(|r| RKT_IMAGES_GAUGE.set(r.get_ref().images.len() as f64))
        })
        .inspect(move |_| {
            TRACE.with(|t| {
                let mut span = t.take().unwrap();
                span.log(|log| {
                    log.std().event("gRPC call");
                });
                t.set(Some(span));
            })
        })
        .map_err(|e| println!("{:?}", e))
        .inspect(move |_| TRACE.with(|t| t.set(None)))
        .for_each(|_| Ok(()));

    // Spawn client task, it sends a GRPC request on each SIGQUIT.
    ensure!(core.execute(grpc_request).is_ok(), "gRPC task error");
    let own_pid = nix::unistd::getpid();
    println!("Running! Trigger with: kill -s SIGQUIT {}", own_pid);

    // Spawn the `Metrics` service, it serves `/metrics` for Prometheus.
    let metrics_addr = "127.0.0.1:33333".parse()?;
    let listener = TcpListener::bind(&metrics_addr, &core.handle())?;
    let handle3 = core.handle();
    core.handle().spawn(
        listener
            .incoming()
            .for_each(move |(sock, metrics_addr)| {
                Http::new().bind_connection(&handle3, sock, metrics_addr, Metrics);
                Ok(())
            })
            .map_err(|_| ()),
    );

    // Keep running until SIGINT (ctrl-c).
    let ctrl_c = tokio_signal::ctrl_c(&core.handle())
        .flatten_stream()
        .into_future();
    ensure!(core.run(ctrl_c).is_ok(), "SIGINT stream error");

    Ok(())
}
