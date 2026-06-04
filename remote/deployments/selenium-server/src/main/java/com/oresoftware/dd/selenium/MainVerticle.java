package com.oresoftware.dd.selenium;

import com.oresoftware.dd.selenium.handlers.MetricsHandler;
import com.oresoftware.dd.selenium.handlers.RunHandler;
import com.oresoftware.dd.selenium.run.ScenarioRunner;
import io.vertx.core.AbstractVerticle;
import io.vertx.core.Handler;
import io.vertx.core.Promise;
import io.vertx.core.WorkerExecutor;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.core.json.JsonArray;
import io.vertx.core.json.JsonObject;
import io.vertx.ext.web.Router;
import io.vertx.ext.web.RoutingContext;
import io.vertx.ext.web.handler.BodyHandler;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.time.Instant;
import java.util.UUID;
import java.util.function.Supplier;

/**
 * The main Vert.x HTTP verticle for dd-selenium-server.
 *
 * <p>Every endpoint is registered twice: once at the root (consumed in-cluster and by probes) and
 * once under {@code /selenium/...} so the nginx gateway can proxy the prefixed path through
 * unchanged, exactly like dd-web-scraper ({@code /scrape}) and dd-browser-test-server
 * ({@code /browser-test}).
 */
public final class MainVerticle extends AbstractVerticle {

  private static final Logger log = LoggerFactory.getLogger(MainVerticle.class);

  private static final String SERVER_STARTED_AT = Instant.now().toString();
  private static final String SERVER_INSTANCE_ID = UUID.randomUUID().toString();

  private final Config config;
  private final ScenarioRunner runner;
  private final WorkerExecutor executor;
  private final Auth auth;

  public MainVerticle(final Config config, final ScenarioRunner runner, final WorkerExecutor executor) {
    this.config = config;
    this.runner = runner;
    this.executor = executor;
    this.auth = new Auth(config);
  }

  @Override
  public void start(final Promise<Void> startPromise) {

    final Router router = Router.router(vertx);
    router.route().handler(BodyHandler.create().setBodyLimit(2L * 1024L * 1024L));

    json(router, "/", this::serviceDescriptor);
    json(router, "/selenium", this::serviceDescriptor);
    json(router, "/tools", this::toolsDescriptor);
    json(router, "/selenium/tools", this::toolsDescriptor);
    json(router, "/status", this::statusDescriptor);
    json(router, "/selenium/status", this::statusDescriptor);
    json(router, "/healthz", this::healthDescriptor);
    json(router, "/selenium/healthz", this::healthDescriptor);

    router.get("/readyz").handler(ctx -> ctx.response()
        .putHeader("content-type", "application/json")
        .end("{\"status\":\"ready\"}"));

    router.get("/metrics").handler(MetricsHandler.create());
    router.get("/selenium/metrics").handler(MetricsHandler.create());

    final RunHandler runHandler = new RunHandler(config, runner, executor);
    final Handler<RoutingContext> authGate = ctx -> {
      if (auth.isAuthorized(ctx)) {
        ctx.next();
      } else {
        ctx.response()
            .setStatusCode(401)
            .putHeader("content-type", "application/json")
            .end("{\"ok\":false,\"error\":\"unauthorized\"}");
      }
    };
    router.post("/run").handler(authGate).handler(runHandler);
    router.post("/selenium/run").handler(authGate).handler(runHandler);

    router.errorHandler(500, ctx -> {
      log.error("Unhandled error on {}: {}", ctx.request().path(),
          ctx.failure() == null ? "n/a" : ctx.failure().toString(), ctx.failure());
      if (!ctx.response().ended()) {
        ctx.response()
            .setStatusCode(500)
            .putHeader("content-type", "application/json")
            .end("{\"ok\":false,\"error\":\"internal_server_error\"}");
      }
    });

    final String host = System.getenv().getOrDefault("HTTP_HOST", config.httpHost);
    final HttpServerOptions httpOptions = new HttpServerOptions()
        .setLogActivity(false)
        .setIdleTimeout(300);

    vertx.createHttpServer(httpOptions)
        .requestHandler(router)
        .listen(config.httpPort, host)
        .onSuccess(server -> {
          log.info("dd-selenium-server listening on {}:{} (grid={})",
              host, server.actualPort(), config.remoteUrl);
          startPromise.complete();
        })
        .onFailure(err -> {
          log.error("dd-selenium-server failed to bind {}:{}", host, config.httpPort, err);
          startPromise.fail(err);
        });
  }

  private static void json(final Router router, final String path, final Supplier<JsonObject> body) {
    router.get(path).handler(ctx -> ctx.response()
        .putHeader("content-type", "application/json")
        .end(body.get().encode()));
  }

  private JsonObject serviceDescriptor() {
    return new JsonObject()
        .put("service", "dd-selenium-server")
        .put("ok", true)
        .put("endpoints", new JsonObject()
            .put("run", "POST /run")
            .put("tools", "GET /selenium/tools")
            .put("status", "GET /selenium/status")
            .put("healthz", "GET /selenium/healthz")
            .put("metrics", "GET /selenium/metrics"))
        .put("tool", "selenium")
        .put("browserHeadless", config.browserHeadless)
        .put("allowEvaluate", config.allowEvaluate);
  }

  private JsonObject toolsDescriptor() {
    return new JsonObject()
        .put("default", "selenium")
        .put("tools", new JsonArray().add(new JsonObject()
            .put("name", "selenium")
            .put("version", seleniumVersion())
            .put("supportsHeadless", true)
            .put("supportsEvaluate", config.allowEvaluate)));
  }

  private JsonObject statusDescriptor() {
    return new JsonObject()
        .put("ok", true)
        .put("service", "dd-selenium-server")
        .put("serverStartedAt", SERVER_STARTED_AT)
        .put("serverInstanceId", SERVER_INSTANCE_ID)
        .put("inFlight", runner.inFlight())
        .put("maxConcurrent", config.maxConcurrent)
        .put("defaultTool", "selenium")
        .put("defaultTimeoutMs", config.defaultTimeoutMs)
        .put("maxTimeoutMs", config.maxTimeoutMs)
        .put("maxSteps", config.maxSteps)
        .put("browserHeadless", config.browserHeadless)
        .put("allowEvaluate", config.allowEvaluate)
        .put("remoteUrl", config.remoteUrl);
  }

  private JsonObject healthDescriptor() {
    return new JsonObject()
        .put("ok", true)
        .put("service", "dd-selenium-server")
        .put("serverStartedAt", SERVER_STARTED_AT)
        .put("serverInstanceId", SERVER_INSTANCE_ID)
        .put("inFlight", runner.inFlight());
  }

  private static String seleniumVersion() {
    try {
      final String version =
          org.openqa.selenium.remote.RemoteWebDriver.class.getPackage().getImplementationVersion();
      return (version == null || version.isBlank()) ? "unknown" : version;
    } catch (Throwable t) {
      return "unknown";
    }
  }
}
