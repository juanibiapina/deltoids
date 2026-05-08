variable "region" {
  type    = string
  default = "us-east-1"
}

resource "aws_s3_bucket" "logs" {
  bucket = "my-app-logs"
  acl    = "log-delivery-write"

  routes = [
    {
      name  = "datadog"
      value = "new"
    },
    {
      name  = "grafana"
      value = "two"
    },
  ]
}

module "vpc" {
  source = "terraform-aws-modules/vpc/aws"
  cidr   = "10.0.0.0/16"
}
