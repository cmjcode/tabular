FROM ubuntu:noble

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    debhelper \
    devscripts \
    cargo \
    rustc \
    pkg-config \
    libssl-dev \
    libgtk-3-dev \
    libxcb1-dev \
    libxkbcommon-dev \
    libglib2.0-dev \
    libudev-dev \
    libpango1.0-dev \
    libatk1.0-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a small working dir for builds, we'll mount the project at /build at runtime
WORKDIR /build

# Keep image small: no ENTRYPOINT, we'll run commands explicitly from the host script
