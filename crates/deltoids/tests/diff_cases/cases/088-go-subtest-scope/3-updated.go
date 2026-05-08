package mypkg

import "testing"

func TestUserService(t *testing.T) {
	t.Run("creates a user", func(t *testing.T) {
		scope := map[string]string{
			"tenantID": "tenant-1",
			"region":   "eu",
			"role":     "user",
			"locale":   "en",
		}
		_ = scope
	})
}
