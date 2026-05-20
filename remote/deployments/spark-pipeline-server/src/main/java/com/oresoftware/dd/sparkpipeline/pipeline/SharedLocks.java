package com.oresoftware.dd.sparkpipeline.pipeline;

import org.ores.async.NeoLock;

/**
 * Process-wide async mutexes used by {@link CompositionDemoPipeline}.
 *
 * <p>{@link NeoLock} is async.java's callback-based mutex — unlike {@code synchronized} or
 * {@link java.util.concurrent.locks.ReentrantLock}, the thread that calls {@link NeoLock#acquire}
 * is not blocked, and the {@code Unlock} token can be released by any thread. That makes it
 * the right primitive for serialising shared-resource access in a callback-driven pipeline
 * where the "acquire" and "release" frequently happen on different Vert.x worker threads.
 */
final class SharedLocks {

  private SharedLocks() {
  }

  /**
   * Shared lock that the {@code COMPOSITION_DEMO} job kind uses to serialise updates to the
   * synthetic "publication count" counter. Multiple concurrent demo jobs all contend on this
   * lock when they reach the {@code publishCount} stage, demonstrating async-mutex usage in a
   * realistic-shaped pipeline.
   */
  static final NeoLock PUBLICATION_LOCK = new NeoLock("composition-demo:publish-count");
}
