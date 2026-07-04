RSpec.describe Client do
  it "falls back to a default" do
    setup_stub
    expect(result).to eq("default")
  end

  it "exposes the total count" do
    expect(client.total).to eq(1)
  end
end
