resource "aws_s3_bucket" "data" {
  bucket = "data"
}

# New docs
resource "aws_s3_bucket" "logs" {
  bucket = "logs"
}

resource "aws_s3_bucket" "other" {
  bucket = "other"
}
