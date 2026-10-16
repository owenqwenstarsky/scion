# ADR 0002: JSON on the TLS port

`proto/scion/v1/scion.proto` records the gRPC shape. The server that actually runs speaks JSON under `/v1`. The CLI (`reqwest`) and the Next.js backend-for-frontend both use that API. Tonic is a dependency of the server crate but its router is not mounted; wiring `Routes::into_axum_router` was left out so the tested contract stayed one HTTP stack.

Sessions are `X-Scion-Session` plus `X-Scion-Csrf`. The web service token is `Authorization: Bearer` and cannot act as a user by itself.
