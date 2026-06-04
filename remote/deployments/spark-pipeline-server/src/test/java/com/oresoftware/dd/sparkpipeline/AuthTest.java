package com.oresoftware.dd.sparkpipeline;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AuthTest {

  @Test
  void constantTimeEqualsMatchesEqualStrings() {
    assertTrue(Auth.constantTimeEquals("shared-secret", "shared-secret"));
  }

  @Test
  void constantTimeEqualsRejectsDifferentStrings() {
    assertFalse(Auth.constantTimeEquals("shared-secret", "other-secret"));
  }
}
