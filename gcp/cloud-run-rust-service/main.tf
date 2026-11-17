locals {
  contract_labels = {
    "managed-by"       = "terraform"
    "runtime"          = "rust"
    "service-contract" = "ores-v1"
  }
}

resource "google_cloud_run_v2_service" "this" {
  provider = google-beta

  project             = var.project_id
  name                = var.name
  location            = var.region
  launch_stage        = "BETA"
  deletion_protection = var.deletion_protection
  ingress             = var.ingress
  labels              = merge(local.contract_labels, var.labels)

  template {
    service_account                  = var.service_account
    timeout                          = "${var.request_timeout_seconds}s"
    max_instance_request_concurrency = var.max_instance_request_concurrency

    scaling {
      min_instance_count = var.min_instance_count
      max_instance_count = var.max_instance_count
    }

    containers {
      image = var.image

      ports {
        name           = "http1"
        container_port = var.container_port
      }

      resources {
        limits = {
          cpu    = var.cpu
          memory = var.memory
        }
        cpu_idle          = true
        startup_cpu_boost = true
      }

      dynamic "env" {
        for_each = var.environment

        content {
          name  = env.key
          value = env.value
        }
      }

      # Cloud Run can route traffic immediately after startup succeeds. Use the
      # fail-closed readiness route here so a new instance is not admitted while
      # configuration, migrations, dependencies, or drain state are unsafe.
      startup_probe {
        initial_delay_seconds = 0
        timeout_seconds       = 2
        period_seconds        = 5
        failure_threshold     = 24

        http_get {
          path = "/readyz"
          port = var.container_port
        }
      }

      # Liveness is process-only. It must not depend on databases, queues,
      # identity providers, object stores, or other downstream services.
      liveness_probe {
        initial_delay_seconds = 10
        timeout_seconds       = 2
        period_seconds        = 10
        failure_threshold     = 3

        http_get {
          path = "/healthz"
          port = var.container_port
        }
      }

      # Readiness removes an unhealthy instance from traffic without restarting
      # it, then restores the instance after /readyz returns success again.
      readiness_probe {
        timeout_seconds   = 2
        period_seconds    = 5
        failure_threshold = 2
        success_threshold = 1

        http_get {
          path = "/readyz"
          port = var.container_port
        }
      }
    }
  }

  lifecycle {
    precondition {
      condition     = var.max_instance_count >= var.min_instance_count
      error_message = "max_instance_count must be greater than or equal to min_instance_count"
    }
  }
}
