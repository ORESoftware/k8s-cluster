variable "aws_region" {
  description = "AWS region for the Airbyte S3 bucket."
  type        = string
  default     = "us-east-1"
}

variable "bucket_name" {
  description = "Globally unique S3 bucket name used by Airbyte."
  type        = string
  default     = "dd-remote-dev-airbyte"
}

variable "iam_user_name" {
  description = "IAM user name for Airbyte S3 credentials."
  type        = string
  default     = "dd-remote-dev-airbyte"
}

variable "secrets_manager_secret_name" {
  description = "AWS Secrets Manager secret that stores Airbyte S3 credentials for External Secrets."
  type        = string
  default     = "dd/remote-dev/airbyte-s3"
}

variable "environment" {
  description = "Environment tag value."
  type        = string
  default     = "remote-dev"
}

variable "force_destroy" {
  description = "Whether Terraform may delete the bucket while it contains objects."
  type        = bool
  default     = false
}

variable "tags" {
  description = "Additional tags to apply to the bucket."
  type        = map(string)
  default     = {}
}
