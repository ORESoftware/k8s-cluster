resource "aws_s3_bucket" "airbyte" {
  bucket        = var.bucket_name
  force_destroy = var.force_destroy

  tags = merge(var.tags, {
    Name        = var.bucket_name
    Component   = "airbyte"
    Environment = var.environment
    ManagedBy   = "terraform"
  })
}

resource "aws_s3_bucket_public_access_block" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_versioning" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id

  rule {
    id     = "abort-incomplete-multipart-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

data "aws_iam_policy_document" "airbyte" {
  statement {
    sid    = "DenyInsecureTransport"
    effect = "Deny"

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.airbyte.arn,
      "${aws_s3_bucket.airbyte.arn}/*",
    ]

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "airbyte" {
  bucket = aws_s3_bucket.airbyte.id
  policy = data.aws_iam_policy_document.airbyte.json

  depends_on = [aws_s3_bucket_public_access_block.airbyte]
}
