describe("UserService", function()
  it("creates a user", function()
    local scope = {
      tenant_id = "tenant-1",
      region = "eu",
      role = "user",
      locale = "en",
    }
    assert.is_not_nil(scope)
  end)
end)
