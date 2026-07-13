output "bucket_arn" {
  description = "ARN of the Airbyte S3 bucket."
  value       = aws_s3_bucket.airbyte.arn
}

output "bucket_name" {
  description = "Name of the Airbyte S3 bucket."
  value       = aws_s3_bucket.airbyte.bucket
}

output "bucket_region" {
  description = "AWS region where the Airbyte S3 bucket is managed."
  value       = var.aws_region
}

output "iam_user_arn" {
  description = "IAM user ARN for Airbyte S3 access."
  value       = aws_iam_user.airbyte.arn
}

output "secrets_manager_secret_name" {
  description = "Secrets Manager secret containing Airbyte S3 credentials."
  value       = aws_secretsmanager_secret.airbyte_s3.name
}
