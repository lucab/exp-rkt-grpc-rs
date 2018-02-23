# Cloud-native Rust experiment

BEWARE: This is just a personal quick and dirty experiment to play with new libraries. Abandon all hope, you who enter here.

This repository contains a simple Rust program to learn and play with cloud-native protocols and libries.

`exp-rkt-grpc` is a command-line application which keeps running until interrupted (via `ctrl-c`). Its core logic consists of an event loop which:
 * reacts to SIGQUIT signal (aka `ctrl-\`)
 * sends a `ListImages` request to [rkt api-service][rkt-grpc]
 * registers the number of images in rkt store as an internal metric
 * traces the whole event lifespan and sends traces to Jaeger upon completion
 * exposes process own metrics in Prometheus format

It uses the following libraries:
 * async event loop: [tokio][tokio]
 * protobuf codegen: [prost][prost]
 * grpc client/server support: [tower-grpc][tower-grpc]
 * Prometheus metrics: [rust-prometheus][rust-prometheus]
 * OpenTracing and Jaeger support: [rustracing][rustracing]

[rkt-grpc]: https://github.com/rkt/rkt/blob/master/Documentation/subcommands/api-service.md
[tokio]: https://tokio.rs/
[prost]: https://github.com/danburkert/prost
[tower-grpc]: https://github.com/tower-rs/tower-grpc
[rust-prometheus]: https://github.com/pingcap/rust-prometheus
[rustracing]: https://github.com/sile/rustracing

# Demo

Start external containers/services:
```shell
sudo systemd-run `which rkt` run --insecure-options=image --port 5775-udp:5775 --port 6831-udp:6831 --port 6832-udp:6832 --port 5778-tcp:5778 --port 16686-tcp:16686 --port 14268-tcp:14268 docker://jaegertracing/all-in-one
sudo systemd-run `which rkt` api-service
```

Build and run the binary via cargo, then check idle-state metrics:
```shell
cargo run &
PID=$!
curl http://127.0.0.1:33333/metrics
```

Trigger an event and check metrics:
```shell
kill -s SIGQUIT $PID
curl http://127.0.0.1:33333/metrics
```

Fetch an additional image and check updated metrics:
```shell
sudo rkt fetch --trust-keys-from-https quay.io/coreos/matchbox:v0.7.0
kill -s SIGQUIT $PID
curl http://127.0.0.1:33333/metrics
```

Explore traces reported to Jaeger:
```
xdg-open http://127.0.0.1:16686
```

# Demo environment

This project *should* run on any modern GNU/Linux system, and the above demo requires:
 * cargo/rustc (aha!)
 * systemd-run
 * rkt
 * xdg-open / browser

