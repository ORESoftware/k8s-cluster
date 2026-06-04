package com.oresoftware.dd.akkaws.pipeline;

import akka.actor.testkit.typed.javadsl.ActorTestKit;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AkkaStreamsPipelineTest {

  private static ActorTestKit testKit;
  private static AkkaStreamsPipeline pipeline;

  @BeforeAll
  static void setUp() {
    testKit = ActorTestKit.create();
    pipeline = new AkkaStreamsPipeline(testKit.system());
  }

  @AfterAll
  static void tearDown() {
    testKit.shutdownTestKit();
  }

  @Test
  void happyPath() throws Exception {
    final String out = pipeline.process("{\"id\":\"abc\",\"payload\":\"hi\"}")
        .toCompletableFuture().get(5, TimeUnit.SECONDS);
    assertTrue(out.contains("\"score\""), () -> "expected score field in: " + out);
    assertTrue(out.contains("\"lookupA\""));
    assertTrue(out.contains("\"lookupB\""));
  }

  @Test
  void rejectsMalformedJson() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("not json").toCompletableFuture().get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause() instanceof Exception);
  }

  @Test
  void rejectsMissingRequiredField() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("{\"payload\":\"missing id\"}").toCompletableFuture()
            .get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause().getMessage().contains("missing required field `id`"));
  }

  @Test
  void poisonPillSurfacesAsScoreFailure() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("{\"id\":\"poison\",\"payload\":\"x\"}").toCompletableFuture()
            .get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause().getMessage().contains("poison-pill"),
        () -> "expected poison-pill in: " + ex.getCause().getMessage());
  }
}
