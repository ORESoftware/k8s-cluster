# --- build stage -------------------------------------------------------------
FROM docker.io/library/maven:3.9.9-eclipse-temurin-17 AS build

# The build expects the k8s-cluster repo root as its build context (so it can reach
# remote/libs/pg-defs/generated/jvm/jooq for the jOOQ Tables source set). Build the image with:
#
#     docker build -f remote/spark-pipeline-server/Dockerfile -t dd-spark-pipeline-server:dev .
#
# from the k8s-cluster repo root.
WORKDIR /workspace

# Copy the POM and the generated pg-defs jOOQ sources first, so dependency resolution warms a
# distinct layer that can be reused across source-only changes.
COPY remote/spark-pipeline-server/pom.xml ./remote/spark-pipeline-server/pom.xml
COPY remote/libs/pg-defs/generated/jvm/jooq ./remote/libs/pg-defs/generated/jvm/jooq

WORKDIR /workspace/remote/spark-pipeline-server
RUN mvn -B -e -DskipTests dependency:go-offline || true

# Copy spark-pipeline-server sources and build the shaded jar.
COPY remote/spark-pipeline-server/src ./src
RUN mvn -B -e -DskipTests package

# --- runtime stage -----------------------------------------------------------
FROM docker.io/library/eclipse-temurin:17-jre

RUN groupadd --system pipeline && useradd --system --gid pipeline --uid 10001 pipeline

WORKDIR /opt/dd-spark-pipeline-server
COPY --from=build /workspace/remote/spark-pipeline-server/target/dd-spark-pipeline-server.jar /opt/dd-spark-pipeline-server/app.jar

USER 10001:10001

ENV HTTP_HOST=0.0.0.0
ENV HTTP_PORT=8085
ENV PIPELINE_MAX_CONCURRENT=4
ENV JAVA_OPTS="-XX:+UseG1GC -XX:MaxRAMPercentage=70 -Dfile.encoding=UTF-8"

EXPOSE 8085

ENTRYPOINT ["/bin/sh", "-lc", "exec java $JAVA_OPTS -jar /opt/dd-spark-pipeline-server/app.jar"]
