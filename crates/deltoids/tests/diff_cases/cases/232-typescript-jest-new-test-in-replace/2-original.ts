describe("client", () => {
  it("falls back to a default", () => {
    setupStub();
    expect(result).toEqual("default");
  });

  it("exposes the total count", () => {
    expect(client.total).toEqual(1);
  });
});
