package com.oresoftware.dd.selenium.run;

import com.oresoftware.dd.selenium.Config;
import io.micrometer.core.instrument.Gauge;
import io.micrometer.core.instrument.MeterRegistry;
import io.vertx.core.json.Json;
import io.vertx.core.json.JsonArray;
import io.vertx.core.json.JsonObject;
import io.vertx.micrometer.backends.BackendRegistries;
import org.openqa.selenium.By;
import org.openqa.selenium.Dimension;
import org.openqa.selenium.JavascriptExecutor;
import org.openqa.selenium.Keys;
import org.openqa.selenium.NoSuchElementException;
import org.openqa.selenium.OutputType;
import org.openqa.selenium.TakesScreenshot;
import org.openqa.selenium.WebElement;
import org.openqa.selenium.chrome.ChromeOptions;
import org.openqa.selenium.interactions.Actions;
import org.openqa.selenium.logging.LogEntries;
import org.openqa.selenium.logging.LogEntry;
import org.openqa.selenium.logging.LogType;
import org.openqa.selenium.logging.LoggingPreferences;
import org.openqa.selenium.remote.RemoteWebDriver;
import org.openqa.selenium.support.ui.ExpectedConditions;
import org.openqa.selenium.support.ui.Select;
import org.openqa.selenium.support.ui.WebDriverWait;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.net.URL;
import java.time.Duration;
import java.time.Instant;
import java.util.Arrays;
import java.util.Base64;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.logging.Level;

/**
 * Executes a bounded scenario against the in-pod Selenium Grid over {@link RemoteWebDriver}.
 *
 * <p>Every call to {@link #run} opens a fresh remote session (so cookies and storage do not leak
 * between scenarios), walks the declarative steps, then quits the session. WebDriver is blocking,
 * so callers must invoke {@link #run} from a worker thread, never the Vert.x event loop. Concurrency
 * is gated through {@link #tryAcquire()} / {@link #release()} so the pod never runs more than
 * {@code SELENIUM_MAX_CONCURRENT} browser sessions at once.
 */
public final class ScenarioRunner {

  private static final Logger log = LoggerFactory.getLogger(ScenarioRunner.class);

  private final Config config;
  private final AtomicInteger inFlight = new AtomicInteger(0);
  private final MeterRegistry registry;

  public ScenarioRunner(final Config config) {
    this.config = config;

    MeterRegistry resolved = null;
    try {
      resolved = BackendRegistries.getDefaultNow();
    } catch (Exception ignore) {
      // Metrics backend not ready; /metrics still serves Vert.x defaults.
    }
    this.registry = resolved;
    if (registry != null) {
      Gauge.builder("selenium_in_flight", inFlight, AtomicInteger::get)
          .description("Current in-flight Selenium scenarios.")
          .register(registry);
    }
  }

  public int inFlight() {
    return inFlight.get();
  }

  /** Reserve a concurrency slot; returns false when the per-pod cap is already reached. */
  public boolean tryAcquire() {
    while (true) {
      final int current = inFlight.get();
      if (current >= config.maxConcurrent) {
        return false;
      }
      if (inFlight.compareAndSet(current, current + 1)) {
        return true;
      }
    }
  }

  public void release() {
    inFlight.decrementAndGet();
  }

  public JsonObject run(final JsonObject input, final String requestId, final String startedAtIso) {
    final long startMs = System.currentTimeMillis();
    final JsonArray stepsLog = new JsonArray();
    final JsonObject extracted = new JsonObject();
    final JsonArray screenshots = new JsonArray();
    final JsonArray consoleEntries = new JsonArray();
    final JsonArray pageErrors = new JsonArray();

    boolean ok = true;
    String error = null;
    String finalUrl = null;
    String finalTitle = null;

    RemoteWebDriver driver = null;
    try {
      driver = newDriver(input);
      final JsonArray steps = input.getJsonArray("steps", new JsonArray());

      // Optional opening goto: keep the scenario declarative when a top-level url is supplied and
      // the first step isn't already a goto.
      final String openingUrl = input.getString("url");
      final JsonObject firstStep = steps.isEmpty() ? null : steps.getJsonObject(0);
      if (openingUrl != null && firstStep != null && !"goto".equals(firstStep.getString("action"))) {
        driver.manage().timeouts().pageLoadTimeout(Duration.ofMillis(config.stepTimeoutMs));
        driver.get(openingUrl);
      }

      for (int i = 0; i < steps.size(); i++) {
        final JsonObject step = steps.getJsonObject(i);
        final String action = step.getString("action");
        final long stepStart = System.currentTimeMillis();
        final long stepTimeout = step.containsKey("timeoutMs")
            ? step.getLong("timeoutMs")
            : config.stepTimeoutMs;
        try {
          runStep(driver, step, extracted, screenshots, stepTimeout);
          stepsLog.add(stepLog(i, action, "ok", System.currentTimeMillis() - stepStart,
              step.getString("description"), null));
        } catch (Exception stepErr) {
          final String msg = messageOf(stepErr);
          stepsLog.add(stepLog(i, action, "error", System.currentTimeMillis() - stepStart,
              step.getString("description"), msg));
          ok = false;
          error = "step " + i + " (" + action + ") failed: " + msg;
          break;
        }
      }

      if (ok && input.getBoolean("captureFinalScreenshot", true)) {
        try {
          final JsonObject shot = captureScreenshot(driver, "final");
          if (shot != null) {
            screenshots.add(shot);
          }
        } catch (Exception screenshotErr) {
          log.warn("final screenshot failed for request {}", requestId, screenshotErr);
        }
      }

      try {
        finalUrl = driver.getCurrentUrl();
      } catch (Exception ignore) {
        // best effort
      }
      try {
        finalTitle = driver.getTitle();
      } catch (Exception ignore) {
        // best effort
      }
      collectConsole(driver, consoleEntries);

      if (ok && input.getBoolean("failOnConsoleError", false)) {
        final boolean sawError = consoleEntries.stream()
            .anyMatch(entry -> "error".equals(((JsonObject) entry).getString("level")));
        if (sawError) {
          ok = false;
          error = "failOnConsoleError: page emitted at least one console error";
        }
      }
    } catch (Exception fatal) {
      ok = false;
      error = messageOf(fatal);
    } finally {
      if (driver != null) {
        try {
          driver.quit();
        } catch (Exception ignore) {
          // session may already be gone
        }
      }
    }

    final long durationMs = System.currentTimeMillis() - startMs;
    recordRun(ok, durationMs);

    final JsonObject result = new JsonObject()
        .put("ok", ok)
        .put("requestId", requestId)
        .put("tool", "selenium")
        .put("durationMs", durationMs)
        .put("startedAt", startedAtIso)
        .put("finishedAt", Instant.now().toString())
        .put("steps", stepsLog)
        .put("extracted", extracted)
        .put("screenshots", screenshots)
        .put("consoleEntries", consoleEntries)
        .put("pageErrors", pageErrors);
    if (finalUrl != null) {
      result.put("finalUrl", finalUrl);
    }
    if (finalTitle != null) {
      result.put("finalTitle", finalTitle);
    }
    if (error != null) {
      result.put("error", error);
    }
    return result;
  }

  private RemoteWebDriver newDriver(final JsonObject input) throws Exception {
    final ChromeOptions options = new ChromeOptions();
    if (config.browserHeadless) {
      options.addArguments("--headless=new");
    }
    options.addArguments("--no-sandbox", "--disable-dev-shm-usage");
    final String userAgent = input.getString("userAgent");
    if (userAgent != null && !userAgent.isBlank()) {
      options.addArguments("--user-agent=" + userAgent);
    }
    try {
      final LoggingPreferences logs = new LoggingPreferences();
      logs.enable(LogType.BROWSER, Level.ALL);
      options.setCapability("goog:loggingPrefs", logs);
    } catch (Exception ignore) {
      // logging prefs are best-effort
    }

    final RemoteWebDriver driver = new RemoteWebDriver(new URL(config.remoteUrl), options);
    final JsonObject viewport = input.getJsonObject("viewport");
    if (viewport != null) {
      driver.manage().window().setSize(
          new Dimension(viewport.getInteger("width", 1280), viewport.getInteger("height", 800)));
    }
    driver.manage().timeouts().scriptTimeout(Duration.ofMillis(config.stepTimeoutMs));
    return driver;
  }

  private void runStep(
      final RemoteWebDriver driver,
      final JsonObject step,
      final JsonObject extracted,
      final JsonArray screenshots,
      final long timeoutMs) throws Exception {

    final String action = step.getString("action");
    if (action == null) {
      throw new IllegalArgumentException("step is missing required \"action\"");
    }

    switch (action) {
      case "goto": {
        driver.manage().timeouts().pageLoadTimeout(Duration.ofMillis(timeoutMs));
        driver.get(requireString(step, "url"));
        return;
      }
      case "click": {
        final WebElement el = findOne(driver, requireString(step, "selector"),
            step.getInteger("nth", 0), timeoutMs);
        new WebDriverWait(driver, Duration.ofMillis(timeoutMs))
            .until(ExpectedConditions.elementToBeClickable(el));
        el.click();
        return;
      }
      case "fill": {
        final WebElement el = findOne(driver, requireString(step, "selector"), 0, timeoutMs);
        el.clear();
        el.sendKeys(step.getString("value", ""));
        return;
      }
      case "select": {
        final WebElement el = findOne(driver, requireString(step, "selector"), 0, timeoutMs);
        final String value = step.getString("value", "");
        try {
          new Select(el).selectByValue(value);
        } catch (Exception fallback) {
          // Not a <select> (or no matching option value); approximate with raw keystrokes the way
          // the dd-browser-test-server Selenium driver does.
          el.sendKeys(value);
        }
        return;
      }
      case "press": {
        final CharSequence keys = mapKey(step.getString("key", ""));
        final String selector = step.getString("selector");
        if (selector != null) {
          findOne(driver, selector, 0, timeoutMs).sendKeys(keys);
        } else {
          new Actions(driver).sendKeys(keys).perform();
        }
        return;
      }
      case "waitForSelector": {
        waitForSelector(driver, requireString(step, "selector"), step.getString("state"), timeoutMs);
        return;
      }
      case "waitForUrl": {
        waitForUrl(driver, requireString(step, "url"), timeoutMs);
        return;
      }
      case "waitForTimeout": {
        final long ms = step.getLong("ms", 0L);
        Thread.sleep(Math.max(0L, Math.min(ms, 60_000L)));
        return;
      }
      case "extractText": {
        final String selector = requireString(step, "selector");
        final WebElement el = findOne(driver, selector, 0, timeoutMs);
        extracted.put(orDefault(step.getString("name"), "text:" + selector), el.getText().trim());
        return;
      }
      case "extractAttribute": {
        final String selector = requireString(step, "selector");
        final String attribute = requireString(step, "attribute");
        final WebElement el = findOne(driver, selector, 0, timeoutMs);
        final String value = el.getAttribute(attribute);
        extracted.put(
            orDefault(step.getString("name"), "attr:" + selector + "@" + attribute),
            value == null ? "" : value);
        return;
      }
      case "screenshot": {
        final JsonObject shot = captureScreenshot(driver,
            orDefault(step.getString("name"), "step-" + System.currentTimeMillis()));
        if (shot != null) {
          screenshots.add(shot);
        }
        return;
      }
      case "evaluate": {
        if (!config.allowEvaluate) {
          throw new IllegalStateException(
              "evaluate steps are disabled (set SELENIUM_ALLOW_EVALUATE=true to enable)");
        }
        final Object value = ((JavascriptExecutor) driver)
            .executeScript("return (function(){" + step.getString("script", "") + "})();");
        extracted.put(orDefault(step.getString("name"), "evaluate"), stringify(value));
        return;
      }
      default:
        throw new IllegalArgumentException("unknown action: " + action);
    }
  }

  private WebElement findOne(
      final RemoteWebDriver driver, final String selector, final int nth, final long timeoutMs) {
    final By by = By.cssSelector(selector);
    new WebDriverWait(driver, Duration.ofMillis(timeoutMs))
        .until(ExpectedConditions.presenceOfElementLocated(by));
    final List<WebElement> elements = driver.findElements(by);
    if (nth >= elements.size()) {
      throw new NoSuchElementException(
          "selenium: selector " + selector + " did not match index " + nth);
    }
    return elements.get(nth);
  }

  private void waitForSelector(
      final RemoteWebDriver driver, final String selector, final String state, final long timeoutMs) {
    final By by = By.cssSelector(selector);
    final WebDriverWait wait = new WebDriverWait(driver, Duration.ofMillis(timeoutMs));
    if ("detached".equals(state) || "hidden".equals(state)) {
      wait.until(d -> {
        final List<WebElement> elements = d.findElements(by);
        if (elements.isEmpty()) {
          return true;
        }
        if ("hidden".equals(state)) {
          return !elements.get(0).isDisplayed();
        }
        return false;
      });
      return;
    }
    final WebElement located = wait.until(ExpectedConditions.presenceOfElementLocated(by));
    if (!"attached".equals(state)) {
      wait.until(ExpectedConditions.visibilityOf(located));
    }
  }

  private void waitForUrl(final RemoteWebDriver driver, final String pattern, final long timeoutMs) {
    final WebDriverWait wait = new WebDriverWait(driver, Duration.ofMillis(timeoutMs));
    if (pattern.length() > 1 && pattern.startsWith("/") && pattern.endsWith("/")) {
      wait.until(ExpectedConditions.urlMatches(pattern.substring(1, pattern.length() - 1)));
    } else {
      wait.until(ExpectedConditions.urlContains(pattern));
    }
  }

  private JsonObject captureScreenshot(final RemoteWebDriver driver, final String name) {
    final byte[] png = ((TakesScreenshot) driver).getScreenshotAs(OutputType.BYTES);
    final boolean truncated = png.length > config.maxScreenshotBytes;
    final byte[] trimmed = truncated ? Arrays.copyOf(png, config.maxScreenshotBytes) : png;
    final JsonObject out = new JsonObject()
        .put("name", name)
        .put("contentType", "image/png")
        .put("base64", Base64.getEncoder().encodeToString(trimmed))
        .put("bytes", png.length);
    if (truncated) {
      out.put("truncated", true);
    }
    return out;
  }

  private void collectConsole(final RemoteWebDriver driver, final JsonArray consoleEntries) {
    try {
      final LogEntries entries = driver.manage().logs().get(LogType.BROWSER);
      for (LogEntry entry : entries) {
        consoleEntries.add(new JsonObject()
            .put("level", entry.getLevel().getName().toLowerCase())
            .put("text", entry.getMessage())
            .put("timestamp", Instant.ofEpochMilli(entry.getTimestamp()).toString()));
      }
    } catch (Exception ignore) {
      // Browser logs aren't portable across every Grid/driver combo; treat absence as empty.
    }
  }

  private void recordRun(final boolean ok, final long durationMs) {
    if (registry == null) {
      return;
    }
    registry.counter("selenium_runs_total", "status", ok ? "ok" : "error").increment();
    registry.timer("selenium_run_duration_ms").record(durationMs, TimeUnit.MILLISECONDS);
  }

  private static JsonObject stepLog(
      final int index,
      final String action,
      final String status,
      final long durationMs,
      final String description,
      final String error) {
    final JsonObject entry = new JsonObject()
        .put("index", index)
        .put("action", action)
        .put("status", status)
        .put("durationMs", durationMs);
    if (description != null) {
      entry.put("description", description);
    }
    if (error != null) {
      entry.put("error", error);
    }
    return entry;
  }

  private static String messageOf(final Throwable t) {
    if (t == null) {
      return "unknown error";
    }
    final String msg = t.getMessage();
    if (msg != null && !msg.isBlank()) {
      return msg;
    }
    return t.getClass().getSimpleName();
  }

  private static String requireString(final JsonObject step, final String key) {
    final String value = step.getString(key);
    if (value == null || value.isBlank()) {
      throw new IllegalArgumentException("step is missing required \"" + key + "\"");
    }
    return value;
  }

  private static String orDefault(final String value, final String fallback) {
    return (value == null || value.isBlank()) ? fallback : value;
  }

  private static String stringify(final Object value) {
    if (value == null) {
      return "";
    }
    if (value instanceof String) {
      return (String) value;
    }
    try {
      return Json.encode(value);
    } catch (Exception e) {
      return String.valueOf(value);
    }
  }

  private static CharSequence mapKey(final String key) {
    switch (key) {
      case "Enter":
      case "Return":
        return Keys.ENTER;
      case "Tab":
        return Keys.TAB;
      case "Escape":
      case "Esc":
        return Keys.ESCAPE;
      case "Backspace":
        return Keys.BACK_SPACE;
      case "Delete":
        return Keys.DELETE;
      case "ArrowUp":
        return Keys.ARROW_UP;
      case "ArrowDown":
        return Keys.ARROW_DOWN;
      case "ArrowLeft":
        return Keys.ARROW_LEFT;
      case "ArrowRight":
        return Keys.ARROW_RIGHT;
      case "Home":
        return Keys.HOME;
      case "End":
        return Keys.END;
      case "PageUp":
        return Keys.PAGE_UP;
      case "PageDown":
        return Keys.PAGE_DOWN;
      case "Space":
        return " ";
      default:
        return key;
    }
  }
}
