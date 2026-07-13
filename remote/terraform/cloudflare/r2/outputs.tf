output "account_id" {
  description = "Cloudflare account ID (R2 S3 endpoint host component)."
  value       = var.account_id
}

output "s3_endpoint" {
  description = "S3-compatible endpoint for SDKs. Set R2_ENDPOINT to this."
  value       = "https://${var.account_id}.r2.cloudflarestorage.com"
}

output "bucket_names" {
  description = "All managed bucket names (the <prefix><key> values)."
  value       = sort([for b in cloudflare_r2_bucket.this : b.name])
}

output "edge_domains" {
  description = "Per-bucket edge-cached custom-domain hostnames (empty until edge_domain is set)."
  value       = { for k, d in cloudflare_r2_custom_domain.this : cloudflare_r2_bucket.this[k].name => d.domain }
}

# R2 derives S3 creds from the token: access_key_id = token id,
# secret_access_key = sha256(token value). Both are sensitive.
output "r2_access_key_id" {
  description = "S3 access key id for R2 (the token id). Set R2_ACCESS_KEY_ID to this."
  value       = var.create_access_token ? cloudflare_account_token.r2[0].id : null
  sensitive   = true
}

output "r2_secret_access_key" {
  description = "S3 secret access key for R2 (sha256 of the token value). Set R2_SECRET_ACCESS_KEY to this."
  value       = var.create_access_token ? sha256(cloudflare_account_token.r2[0].value) : null
  sensitive   = true
}
