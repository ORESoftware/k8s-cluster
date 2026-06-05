provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      ManagedBy  = "terraform"
      Repository = "ORESoftware/k8s-cluster"
    }
  }
}
