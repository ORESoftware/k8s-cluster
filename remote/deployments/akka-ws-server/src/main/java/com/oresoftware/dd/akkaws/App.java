package com.oresoftware.dd.akkaws;

import akka.actor.typed.ActorSystem;
import akka.actor.typed.javadsl.Behaviors;
import akka.http.javadsl.Http;
import akka.http.javadsl.ServerBinding;
import akka.http.javadsl.server.Route;
import com.oresoftware.dd.akkaws.pipeline.AkkaStreamsPipeline;
import com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.concurrent.CompletionStage;
import java.util.concurrent.Executors;

/**
 * Process entry point for dd-akka-ws-server. Boots an Akka ActorSystem, wires up the two
 * pipeline implementations (async.java callback-style + Akka Streams), and binds an HTTP/WS
 * listener that exposes each pipeline as its own endpoint plus a side-by-side benchmark route.
 */
public final class App {

  private static final Logger log = LoggerFactory.getLogger(App.class);

  private App() {
  }

  public static void main(final String[] args) {

    final String host = System.getenv().getOrDefault("HTTP_HOST", "0.0.0.0");
    final int port = parsePort(System.getenv("HTTP_PORT"), 8086);

    final ActorSystem<Void> system = ActorSystem.create(Behaviors.empty(), "dd-akka-ws-server");

    // async.java is callback-style and needs its own executor for the per-task work. We share
    // a virtual-thread executor on JDK 21+ for the cheapest possible per-task overhead; this
    // is the same executor pattern documented in async.java's "Project Loom" readme section.
    final var asyncJavaExec = Executors.newVirtualThreadPerTaskExecutor();
    final var asyncJavaPipeline = new AsyncJavaPipeline(asyncJavaExec);
    final var akkaStreamsPipeline = new AkkaStreamsPipeline(system);

    // Explicit OpenTelemetry SDK (no -javaagent). WsRoutes opens one SERVER span per route.
    final Telemetry telemetry = Telemetry.init();

    final Route routes =
        new WsRoutes(system, asyncJavaPipeline, akkaStreamsPipeline, telemetry).all();

    final CompletionStage<ServerBinding> binding =
        Http.get(system).newServerAt(host, port).bind(routes);

    binding.whenComplete((b, err) -> {
      if (err != null) {
        log.error("dd-akka-ws-server: bind failed at {}:{}", host, port, err);
        system.terminate();
        System.exit(1);
        return;
      }
      log.info("dd-akka-ws-server listening on {}:{}", host, b.localAddress().getPort());
    });

    Runtime.getRuntime().addShutdownHook(new Thread(() -> {
      log.info("dd-akka-ws-server shutting down");
      binding.thenCompose(ServerBinding::unbind).whenComplete((u, e) -> system.terminate());
      asyncJavaExec.shutdown();
      telemetry.close();
    }, "shutdown-hook"));
  }

  private static int parsePort(final String raw, final int fallback) {
    if (raw == null || raw.isBlank()) {
      return fallback;
    }
    try {
      return Integer.parseInt(raw.trim());
    } catch (NumberFormatException nfe) {
      log.warn("invalid HTTP_PORT={}; using {}", raw, fallback);
      return fallback;
    }
  }
}
