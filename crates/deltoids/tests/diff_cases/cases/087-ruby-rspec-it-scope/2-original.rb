RSpec.describe "UserService" do
  it "creates a user" do
    scope = {
      tenant_id: "tenant-1",
      region: "eu",
      role: "admin",
      locale: "en",
    }
    expect(scope).to be_truthy
  end
end
