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

resource "aws_iam_user" "airbyte" {
  name = var.iam_user_name
  path = "/service/airbyte/"

  tags = merge(var.tags, {
    Component   = "airbyte"
    Environment = var.environment
  })
}

resource "aws_iam_access_key" "airbyte" {
  user = aws_iam_user.airbyte.name
}

data "aws_iam_policy_document" "airbyte_user" {
  statement {
    sid    = "AirbyteBucketAccess"
    effect = "Allow"

    actions = [
      "s3:GetBucketLocation",
      "s3:ListBucket",
      "s3:ListBucketMultipartUploads",
    ]

    resources = [aws_s3_bucket.airbyte.arn]
  }

  statement {
    sid    = "AirbyteObjectAccess"
    effect = "Allow"

    actions = [
      "s3:AbortMultipartUpload",
      "s3:DeleteObject",
      "s3:GetObject",
      "s3:ListMultipartUploadParts",
      "s3:PutObject",
    ]

    resources = ["${aws_s3_bucket.airbyte.arn}/*"]
  }
}

resource "aws_iam_user_policy" "airbyte" {
  name   = "${var.iam_user_name}-s3"
  user   = aws_iam_user.airbyte.name
  policy = data.aws_iam_policy_document.airbyte_user.json
}

resource "aws_secretsmanager_secret" "airbyte_s3" {
  name                    = var.secrets_manager_secret_name
  recovery_window_in_days = 30

  tags = merge(var.tags, {
    Component   = "airbyte"
    Environment = var.environment
  })
}

resource "aws_secretsmanager_secret_version" "airbyte_s3" {
  secret_id = aws_secretsmanager_secret.airbyte_s3.id
  secret_string = jsonencode({
    AIRBYTE_S3_ACCESS_KEY_ID     = aws_iam_access_key.airbyte.id
    AIRBYTE_S3_SECRET_ACCESS_KEY = aws_iam_access_key.airbyte.secret
  })
}
