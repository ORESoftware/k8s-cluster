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
