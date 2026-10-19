FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends zip unzip p7zip-full \
    && rm -rf /var/lib/apt/lists/*
