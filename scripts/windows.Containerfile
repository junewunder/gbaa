# The Windows build's toolchain and its check (`just windows-exe`,
# `just windows-check`; docs/research/windows-build.md): Fedora's MinGW-w64
# (the C compiler and linker for x86_64-pc-windows-gnu, and the CMake
# toolchain file SDL3's build uses), a native gcc for build scripts,
# rustup's stable Rust with the Windows target, and Wine to run the .exe.
# wine-common holds the files Wine's first boot installs (its winmd
# metadata among them): without it that boot retries for five minutes.
# The prefix is made here, once, so a run starts at once.
FROM registry.fedoraproject.org/fedora:44
RUN dnf -y install --setopt=install_weak_deps=False \
        mingw64-gcc mingw64-gcc-c++ mingw64-winpthreads-static \
        cmake ninja-build make gcc wine-core.x86_64 wine-common \
    && dnf clean all
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal \
        --default-toolchain stable --target x86_64-pc-windows-gnu --no-modify-path
ENV WINEPREFIX=/opt/wineprefix WINEDEBUG=-all WINEDLLOVERRIDES=mscoree,mshtml=
RUN wine wineboot -i && wineserver -w
