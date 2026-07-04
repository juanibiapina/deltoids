describe("client", () => {
  it("recovers via the slow path", () => {
    setupStub();
    expect(result).toEqual({ githubId: 1, login: "ghost" });
  });

  it("raises when recovery returns nothing", () => {
    setupStub();
    expect(() => run()).toThrow(RecoveryError);
  });

  it("exposes the total count", () => {
    expect(client.total).toEqual(1);
  });
});
