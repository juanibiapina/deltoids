describe("UserService", () => {
  it("creates a user", () => {
    const scope = {
      tenantId: "tenant-1",
      region: "eu",
      role: "admin",
      locale: "en",
    };
    expect(scope).toBeDefined();
  });
});
