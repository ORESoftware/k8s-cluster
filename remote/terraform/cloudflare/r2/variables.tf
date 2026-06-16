variable "account_id" {
  description = "Cloudflare account ID (alexander.d.mills@gmail.com account). Appears in every R2 S3 endpoint."
  type        = string
}

variable "bucket_prefix" {
  description = "Logical namespace for every bucket. R2 buckets are flat per account, so this prefix IS the namespace boundary."
  type        = string
  default     = "ores-k8s-"
}

variable "buckets" {
  description = <<-EOT
    General-purpose buckets to create. The map key is the short name (becomes
    "<bucket_prefix><key>"); the value tunes per-bucket behaviour. Add a bucket
    by adding one entry here — nothing else changes.
  EOT
  type = map(object({
    location      = optional(string, "WNAM")     # WNAM=US-West, ENAM=US-East, WEUR, EEUR, APAC. "" = auto.
    storage_class = optional(string, "Standard") # "Standard" | "InfrequentAccess"
    edge_cache    = optional(bool, false)        # attach to custom domain + cache rule (needs edge_domain set)
  }))
  default = {
    artifacts     = { location = "WNAM" }
    media         = { location = "WNAM", edge_cache = true }
    public-assets = { location = "WNAM", edge_cache = true }
    backups       = { location = "WNAM", storage_class = "InfrequentAccess" }
    cache         = { location = "WNAM" }
    general       = { location = "WNAM" }
  }
}

# ---- Edge caching (parametrized; off until a domain is supplied) ----
variable "edge_domain" {
  description = <<-EOT
    Apex/base domain of a Cloudflare zone in this account, used to mint per-bucket
    custom domains (e.g. "<bucket>.cdn.example.com"). Leave "" to skip all edge-cache
    resources — buckets still work over the S3 API. Set this + edge_zone_id later to
    light up CDN edge caching with zero changes to the buckets themselves.
  EOT
  type        = string
  default     = ""
}

variable "edge_zone_id" {
  description = "Zone ID that owns edge_domain. Required only when edge_domain is set."
  type        = string
  default     = ""
}

variable "edge_subdomain" {
  description = "Label between the bucket name and the zone, e.g. \"cdn\" => <bucket>.cdn.<edge_domain>."
  type        = string
  default     = "cdn"
}

variable "edge_cache_ttl" {
  description = "Edge cache TTL (seconds) applied by the cache rule on the custom-domain hostnames."
  type        = number
  default     = 86400
}

variable "create_access_token" {
  description = "Create an account-scoped R2 API token and output S3-style credentials for the k8s secret."
  type        = bool
  default     = true
}

variable "token_name" {
  description = "Name for the generated R2 API token."
  type        = string
  default     = "ores-k8s-r2-storage"
}
