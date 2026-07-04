package client

import "testing"

func TestClient(t *testing.T) {
	t.Run("recovers via the slow path", func(t *testing.T) {
		setupStub()
		if got := result(); got != "ghost" {
			t.Fatalf("got %q", got)
		}
	})

	t.Run("raises when recovery returns nothing", func(t *testing.T) {
		setupStub()
		if err := run(); err == nil {
			t.Fatal("expected error")
		}
	})

	t.Run("exposes the total count", func(t *testing.T) {
		if got := total(); got != 1 {
			t.Fatalf("got %d", got)
		}
	})
}
