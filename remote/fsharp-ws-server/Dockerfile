# --- build stage -------------------------------------------------------------
# Build context = repo root so this stage can fetch sibling sources if needed.
# Build the image from the k8s-cluster repo root:
#   docker build -f remote/fsharp-ws-server/Dockerfile -t dd-fsharp-ws-server:dev .
FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build

WORKDIR /workspace/remote/fsharp-ws-server

# Restore as its own layer so the NuGet cache survives source-only edits.
COPY remote/fsharp-ws-server/DdFsharpWsServer.fsproj ./DdFsharpWsServer.fsproj
RUN dotnet restore DdFsharpWsServer.fsproj

# Copy the rest and publish a framework-dependent release build. We omit
# `--no-restore` so that any stray host-side `bin/` / `obj/` artefacts that
# slipped past `.dockerignore` don't poison the layer-cached restore from above
# — `publish` will just notice the inputs are unchanged and reuse the cache.
COPY remote/fsharp-ws-server/PipelineStages.fs   ./PipelineStages.fs
COPY remote/fsharp-ws-server/RxPipeline.fs       ./RxPipeline.fs
COPY remote/fsharp-ws-server/AsyncPipeline.fs    ./AsyncPipeline.fs
COPY remote/fsharp-ws-server/BenchmarkRunner.fs  ./BenchmarkRunner.fs
COPY remote/fsharp-ws-server/WsRoutes.fs         ./WsRoutes.fs
COPY remote/fsharp-ws-server/Program.fs          ./Program.fs
RUN dotnet publish DdFsharpWsServer.fsproj \
        -c Release \
        -o /out \
        /p:UseAppHost=false

# --- runtime stage -----------------------------------------------------------
FROM mcr.microsoft.com/dotnet/aspnet:10.0

RUN groupadd --system fsws \
 && useradd --system --gid fsws --uid 10001 fsws

WORKDIR /opt/dd-fsharp-ws-server
COPY --from=build /out ./

USER 10001:10001

ENV HTTP_HOST=0.0.0.0 \
    HTTP_PORT=8087 \
    DOTNET_EnableDiagnostics=0 \
    DOTNET_gcServer=1 \
    DOTNET_TieredPGO=1

EXPOSE 8087

ENTRYPOINT ["dotnet", "/opt/dd-fsharp-ws-server/dd-fsharp-ws-server.dll"]
