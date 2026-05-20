package com.oresoftware.dd.akkaws.pipeline;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.concurrent.ExecutionException;
import java.util.concurrent.Executors;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AsyncJavaPipelineTest {

  private static ExecutorService exec;
  private static AsyncJavaPipeline pipeline;

  @BeforeAll
  static void setUp() {
    exec = Executors.newFixedThreadPool(4);
    pipeline = new AsyncJavaPipeline(exec);
  }

  @AfterAll
  static void tearDown() {
    exec.shutdownNow();
  }

  @Test
  void happyPath() throws Exception {
    final String out = pipeline.process("{\"id\":\"abc\",\"payload\":\"hi\"}")
        .get(5, TimeUnit.SECONDS);
    assertTrue(out.contains("\"score\""), () -> "expected score field in: " + out);
    assertTrue(out.contains("\"lookupA\""));
    assertTrue(out.contains("\"lookupB\""));
  }

  @Test
  void rejectsMalformedJson() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("not json").get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause() instanceof Exception);
  }

  @Test
  void rejectsMissingRequiredField() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("{\"payload\":\"missing id\"}").get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause().getMessage().contains("missing required field `id`"));
  }

  @Test
  void poisonPillSurfacesAsScoreFailure() {
    final var ex = assertThrows(ExecutionException.class,
        () -> pipeline.process("{\"id\":\"poison\",\"payload\":\"x\"}").get(5, TimeUnit.SECONDS));
    assertTrue(ex.getCause().getMessage().contains("poison-pill"),
        () -> "expected poison-pill in: " + ex.getCause().getMessage());
  }
}
