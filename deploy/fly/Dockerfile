# gbaa-server on fly.io (deploy/fly/fly.toml; docs/tools/commands.md,
# "fly-deploy"). Built from the repo root: only the relay crate compiles,
# but cargo needs the whole workspace's manifests.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# The pinned toolchain's no_std target isn't needed here.
RUN rm -f rust-toolchain.toml && cargo build --release --locked -p relay --bin gbaa-server

FROM debian:bookworm-slim
COPY --from=build /src/target/release/gbaa-server /usr/local/bin/gbaa-server
CMD ["gbaa-server", "fly-global-services:7374"]
