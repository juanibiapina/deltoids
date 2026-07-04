RSpec.describe Client do
  it "raises on repeated failure" do
    setup_stub
    expect {
      collect_pages(limit)
      finalize!
    }.to raise_error(RetryError)
  end
end
