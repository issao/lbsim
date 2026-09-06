# Wire diagrams

Tool: draw.io (diagrams.net), stored **uncompressed** as `docs/diagrams/<name>.drawio`.

Every arrow (edge) that represents an API carries custom properties (right-click > Edit Data):

| property | example                      | meaning                                  |
|----------|------------------------------|------------------------------------------|
| proto    | proto/router.proto           | file that defines the API                |
| rpc      | Router.Route                 | service.Method on the arrow              |
| kind     | rpc | stream | event | metric | how the edge is used                    |

Every box has property `component` = the Rust crate, optionally `::module` (e.g. `sim-core::queue`, `sim-ingress`).

Rule: an arrow without `proto`+`rpc` must set `kind=internal` (or `event`) to declare itself
a data-flow hint rather than an API contract.

`tools/check_diagram.py` enforces this: every API-annotated arrow must name a proto file that
exists and a `Service.Method` or message that is declared in it, and every box must carry a
`component`. Run it before committing a diagram change. It caught three real drift problems on
its first run, including an arrow naming a service that the proto did not declare.

Two pages in `system.drawio`:

1. **Data plane and policy boundary** — workload through gateway, router, replica engine and
   KV tier pool, with the policies above and the referee and engine below.
2. **Control plane, engine, product** — the telemetry and autoscaling loops, the engine
   internals, and the real gRPC surface to the dashboard.
