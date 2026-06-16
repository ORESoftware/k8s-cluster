locals {
  # Full bucket name -> config. The prefix is the logical "namespace".
  buckets = { for k, v in var.buckets : k => merge(v, { name = "${var.bucket_prefix}${k}" }) }

  # Buckets that want edge caching AND can have it (a zone/domain was supplied).
  edge_enabled = var.edge_domain != "" && var.edge_zone_id != ""
  edge_buckets = {
    for k, v in local.buckets : k => v if local.edge_enabled && v.edge_cache
  }
}

# ---------------------------------------------------------------------------
# Buckets (general-purpose, list-driven — add a key to var.buckets to scale out)
# ---------------------------------------------------------------------------
resource "cloudflare_r2_bucket" "this" {
  for_each = local.buckets

  account_id    = var.account_id
  name          = each.value.name
  location      = each.value.location != "" ? each.value.location : null
  storage_class = each.value.storage_class
}

# ---------------------------------------------------------------------------
# Edge caching: custom domain per opted-in bucket + a cache rule on the zone.
# Entirely gated on edge_domain/edge_zone_id, so this whole section is inert
# until you point it at a real Cloudflare zone.
# ---------------------------------------------------------------------------
resource "cloudflare_r2_custom_domain" "this" {
  for_each = local.edge_buckets

  account_id  = var.account_id
  bucket_name = cloudflare_r2_bucket.this[each.key].name
  domain      = "${each.key}.${var.edge_subdomain}.${var.edge_domain}"
  zone_id     = var.edge_zone_id
  enabled     = true
  min_tls     = "1.2"
}

# One cache ruleset for the zone: any request to a managed custom-domain
# hostname is edge-cached with an override TTL. Cloudflare's CDN sits in front
# of R2 only on these custom-domain hostnames (the raw *.r2.cloudflarestorage.com
# S3 endpoint is never cached).
resource "cloudflare_ruleset" "r2_edge_cache" {
  count = local.edge_enabled && length(local.edge_buckets) > 0 ? 1 : 0

  zone_id = var.edge_zone_id
  name    = "ores-k8s-r2-edge-cache"
  kind    = "zone"
  phase   = "http_request_cache_settings"

  rules = [
    {
      action      = "set_cache_settings"
      description = "Edge-cache ORES R2 custom-domain buckets"
      enabled     = true
      expression = join(" or ", [
        for k, v in local.edge_buckets :
        format("http.host eq \"%s.%s.%s\"", k, var.edge_subdomain, var.edge_domain)
      ])
      action_parameters = {
        cache = true
        edge_ttl = {
          mode    = "override_origin"
          default = var.edge_cache_ttl
        }
        browser_ttl = {
          mode = "respect_origin"
        }
      }
    }
  ]
}

# ---------------------------------------------------------------------------
# Credentials: account-scoped R2 API token. R2's S3 API derives the
# access-key pair from the token (access_key_id = token id,
# secret_access_key = sha256(token value)).
# ---------------------------------------------------------------------------
# R2 read+write permission group (account-level). Looked up by name so we
# don't hardcode the group UUID.
data "cloudflare_account_api_token_permission_groups_list" "r2" {
  count = var.create_access_token ? 1 : 0

  account_id = var.account_id
  scope      = "com.cloudflare.edge.r2.bucket"
}

resource "cloudflare_account_token" "r2" {
  count = var.create_access_token ? 1 : 0

  account_id = var.account_id
  name       = var.token_name

  policies = [
    {
      effect = "allow"
      permission_groups = [
        for pg in data.cloudflare_account_api_token_permission_groups_list.r2[0].result :
        { id = pg.id } if pg.name == "Workers R2 Storage Write"
      ]
      resources = {
        "com.cloudflare.api.account.${var.account_id}" = "*"
      }
    }
  ]
}
