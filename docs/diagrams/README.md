# Wire diagrams

Tool: draw.io (diagrams.net), stored **uncompressed** as `docs/diagrams/<name>.drawio`.

Every arrow (edge) that represents an API carries custom properties (right-click > Edit Data):

| property | example                      | meaning                                  |
|----------|------------------------------|------------------------------------------|
| proto    | proto/router.proto           | file that defines the API                |
| rpc      | Router.Route                 | service.Method on the arrow              |
| kind     | rpc | stream | event | metric | how the edge is used                    |

Every box has property `component` = Python module path (e.g. `llmsim.router`).

Rule: an arrow without `proto`+`rpc` is a data-flow hint only, not an API contract.
A checker script (later) will verify each `proto`/`rpc` pair exists.
