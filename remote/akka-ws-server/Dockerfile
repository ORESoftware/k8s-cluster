# --- build stage -------------------------------------------------------------
FROM docker.io/library/maven:3.9.9-eclipse-temurin-21 AS build

# Build context = repo root so this stage can fetch sibling sources if needed.
# Build the image from the k8s-cluster repo root:
#   docker build -f remote/akka-ws-server/Dockerfile -t dd-akka-ws-server:dev .
WORKDIR /workspace

# Copy the POM first so Maven warms the dependency cache as its own layer
# (includes the JitPack pull of the patched async.java).
COPY remote/akka-ws-server/pom.xml ./remote/akka-ws-server/pom.xml

WORKDIR /workspace/remote/akka-ws-server
RUN mvn -B -e -DskipTests dependency:go-offline || true

# Copy sources and build the shaded jar.
COPY remote/akka-ws-server/src ./src
RUN mvn -B -e -DskipTests package

# --- runtime stage -----------------------------------------------------------
FROM docker.io/library/eclipse-temurin:21-jre

RUN groupadd --system akkaws && useradd --system --gid akkaws --uid 10001 akkaws

WORKDIR /opt/dd-akka-ws-server
COPY --from=build /workspace/remote/akka-ws-server/target/dd-akka-ws-server.jar /opt/dd-akka-ws-server/app.jar

USER 10001:10001

ENV HTTP_HOST=0.0.0.0
ENV HTTP_PORT=8086
ENV JAVA_OPTS="-XX:+UseG1GC -XX:MaxRAMPercentage=70 -Dfile.encoding=UTF-8"

EXPOSE 8086

ENTRYPOINT ["/bin/sh", "-lc", "exec java $JAVA_OPTS -jar /opt/dd-akka-ws-server/app.jar"]
