# Dockerfile for Interstellar OS build environment
# Using Ubuntu as base to avoid Gentoo masked package issues
FROM ubuntu:22.04

# Non-interactive frontend for apt
ENV DEBIAN_FRONTEND=noninteractive

# Install core build tools, kernel dependencies, GRUB, ISO creation, busybox, git, curl, and Rust
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential \
        gcc \
        make \
        binutils \
        bc \
        flex \
        bison \
        libelf-dev \
        libssl-dev \
        grub-pc-bin \
        xorriso \
        busybox-static \
        nodejs npm \
        curl \
        ca-certificates \
        pkg-config && \
    rm -rf /var/lib/apt/lists/*

# Install Rust toolchain (stable) for building the Tauri UI
# Install Rust toolchain (stable) for building the Tauri UI
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add x86_64-unknown-linux-gnu

# Set working directory inside container
WORKDIR /osaima-build

# Default command drops into a bash shell for interactive use
CMD ["/bin/bash"]
