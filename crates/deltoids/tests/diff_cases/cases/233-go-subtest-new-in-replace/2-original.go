package client

import "testing"

func TestClient(t *testing.T) {
	t.Run("falls back to a default", func(t *testing.T) {
		setupStub()
		if got := result(); got != "default" {
			t.Fatalf("got %q", got)
		}
	})

	t.Run("exposes the total count", func(t *testing.T) {
		if got := total(); got != 1 {
			t.Fatalf("got %d", got)
		}
	})
}
