# --- build stage -------------------------------------------------------------
FROM docker.io/library/maven:3.9.9-eclipse-temurin-17 AS build

WORKDIR /workspace

# Copy the POM first so Maven can warm the dependency cache as a separate layer.
COPY remote/spark-pipeline-server/pom.xml ./pom.xml
RUN mvn -B -e -DskipTests dependency:go-offline || true

# Copy sources and build the shaded jar.
COPY remote/spark-pipeline-server/src ./src
RUN mvn -B -e -DskipTests package

# --- runtime stage -----------------------------------------------------------
FROM docker.io/library/eclipse-temurin:17-jre

RUN groupadd --system pipeline && useradd --system --gid pipeline --uid 10001 pipeline

WORKDIR /opt/dd-spark-pipeline-server
COPY --from=build /workspace/target/dd-spark-pipeline-server.jar /opt/dd-spark-pipeline-server/app.jar

USER 10001:10001

ENV HTTP_HOST=0.0.0.0
ENV HTTP_PORT=8085
ENV PIPELINE_MAX_CONCURRENT=4
ENV JAVA_OPTS="-XX:+UseG1GC -XX:MaxRAMPercentage=70 -Dfile.encoding=UTF-8"

EXPOSE 8085

ENTRYPOINT ["/bin/sh", "-lc", "exec java $JAVA_OPTS -jar /opt/dd-spark-pipeline-server/app.jar"]
