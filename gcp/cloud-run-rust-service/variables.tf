variable "project_id" {
  description = "Google Cloud project that owns the Cloud Run service."
  type        = string

  validation {
    condition     = length(trimspace(var.project_id)) > 0
    error_message = "project_id must not be empty"
  }
}

variable "region" {
  description = "Cloud Run region."
  type        = string
  default     = "us-central1"
}

variable "name" {
  description = "Cloud Run service name."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,47}[a-z0-9]$", var.name))
    error_message = "name must be a valid 2-49 character Cloud Run service name"
  }
}

variable "image" {
  description = "Immutable container image reference. Mutable tags are rejected."
  type        = string

  validation {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.image))
    error_message = "image must be pinned by sha256 digest"
  }
}

variable "service_account" {
  description = "Least-privilege runtime service account email."
  type        = string

  validation {
    condition     = can(regex("^[^@[:space:]]+@[^@[:space:]]+\\.iam\\.gserviceaccount\\.com$", var.service_account))
    error_message = "service_account must be a Google service-account email"
  }
}

variable "container_port" {
  description = "HTTP/1 port exposed by the Rust service."
  type        = number
  default     = 8080

  validation {
    condition     = var.container_port >= 1 && var.container_port <= 65535
    error_message = "container_port must be between 1 and 65535"
  }
}

variable "cpu" {
  description = "Cloud Run CPU limit."
  type        = string
  default     = "1"
}

variable "memory" {
  description = "Cloud Run memory limit."
  type        = string
  default     = "512Mi"
}

variable "min_instance_count" {
  description = "Minimum instances per revision. Zero preserves scale-to-zero."
  type        = number
  default     = 0

  validation {
    condition     = var.min_instance_count >= 0
    error_message = "min_instance_count must not be negative"
  }
}

variable "max_instance_count" {
  description = "Per-revision autoscaling ceiling."
  type        = number
  default     = 10

  validation {
    condition     = var.max_instance_count >= 1
    error_message = "max_instance_count must be at least one"
  }
}

variable "max_instance_request_concurrency" {
  description = "Maximum concurrent requests per instance."
  type        = number
  default     = 80

  validation {
    condition     = var.max_instance_request_concurrency >= 1 && var.max_instance_request_concurrency <= 1000
    error_message = "max_instance_request_concurrency must be between 1 and 1000"
  }
}

variable "request_timeout_seconds" {
  description = "Maximum request duration in seconds."
  type        = number
  default     = 30

  validation {
    condition     = var.request_timeout_seconds >= 1 && var.request_timeout_seconds <= 3600
    error_message = "request_timeout_seconds must be between 1 and 3600"
  }
}

variable "ingress" {
  description = "Cloud Run ingress policy."
  type        = string
  default     = "INGRESS_TRAFFIC_ALL"

  validation {
    condition = contains([
      "INGRESS_TRAFFIC_ALL",
      "INGRESS_TRAFFIC_INTERNAL_ONLY",
      "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER",
    ], var.ingress)
    error_message = "ingress must be a supported Cloud Run v2 ingress value"
  }
}

variable "deletion_protection" {
  description = "Protect the service from accidental Terraform deletion."
  type        = bool
  default     = true
}

variable "environment" {
  description = "Non-secret environment variables. Secrets must use Secret Manager bindings in the caller."
  type        = map(string)
  default     = {}
  sensitive   = false
}

variable "labels" {
  description = "Additional Cloud Run labels."
  type        = map(string)
  default     = {}
}
