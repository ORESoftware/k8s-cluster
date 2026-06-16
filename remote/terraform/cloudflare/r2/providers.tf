# Auth via env so no secret lands in state inputs:
#   export CLOUDFLARE_API_TOKEN=...   (Account token: R2 Admin Read & Write,
#                                      Workers R2 Storage Edit, and — if you set
#                                      edge_domain — Zone/Cache Rules Edit + DNS Edit)
# account_id is passed as a variable (semi-public, appears in every R2 endpoint).
provider "cloudflare" {}
