# --- build stage -------------------------------------------------------------
FROM docker.io/library/maven:3.9.9-eclipse-temurin-17 AS build

# Build context is this deployment directory:
#
#     docker build -f remote/deployments/selenium-server/Dockerfile \
#       -t dd-selenium-server:dev remote/deployments/selenium-server
#
WORKDIR /workspace

# Warm a dependency layer separate from source so source-only changes reuse it.
COPY pom.xml ./pom.xml
RUN mvn -B -e -DskipTests dependency:go-offline || true

COPY src ./src
RUN mvn -B -e -DskipTests package

# --- runtime stage -----------------------------------------------------------
# A plain JRE: this container only runs the Vert.x API and talks to the Selenium
# Grid over HTTP. No Chromium/chromedriver lives here; the Grid owns the browser.
FROM docker.io/library/eclipse-temurin:17-jre

RUN groupadd --system selenium && useradd --system --gid selenium --uid 10001 selenium

WORKDIR /opt/dd-selenium-server
COPY --from=build /workspace/target/dd-selenium-server.jar /opt/dd-selenium-server/app.jar

USER 10001:10001

ENV HTTP_HOST=0.0.0.0 \
    HTTP_PORT=8105 \
    SELENIUM_REMOTE_URL=http://localhost:4444 \
    JAVA_OPTS="-XX:+UseG1GC -XX:MaxRAMPercentage=70 -Dfile.encoding=UTF-8"

EXPOSE 8105

ENTRYPOINT ["/bin/sh", "-lc", "exec java $JAVA_OPTS -jar /opt/dd-selenium-server/app.jar"]
