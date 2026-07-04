RSpec.describe Client do
  it "recovers via the slow path" do
    setup_stub
    expect(result).to eq(github_id: 1, login: "ghost")
  end

  it "raises when recovery returns nothing" do
    setup_stub
    expect { run }.to raise_error(RecoveryError)
  end

  it "retries on transient errors" do
    setup_stub
    expect(client.retries).to eq(3)
  end

  it "logs the recovery attempt" do
    expect(logger).to have_received(:info)
  end

  it "exposes the total count" do
    expect(client.total).to eq(1)
  end
end
