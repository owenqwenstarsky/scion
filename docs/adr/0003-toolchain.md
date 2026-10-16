# ADR 0003: Toolchain pins

- Rust edition 2021, MSRV 1.88, toolchain 1.99.1 (the compiler on the build machine).
- `tower-http` 0.6. The plan named 0.7; 0.6 is the release that resolved with axum 0.8.
- `similar` 3.2 for the merge diff.
- Node 22 and Next.js 16.4 (App Router, `proxy.ts`). TypeScript is pinned to 5.9.3. TypeScript 7.0.2 is published, and this app was not verified with it.
- pnpm 10.33.4. The plan mentioned pnpm 12 via corepack; 10 is what this environment already runs.
- The server image is `debian:bookworm-slim` as uid 65532, not distroless. Distroless was not required to boot the healthcheck.
- Git interop reads and writes loose objects only. `gix` was not added. Packfiles must be unpacked before import.
