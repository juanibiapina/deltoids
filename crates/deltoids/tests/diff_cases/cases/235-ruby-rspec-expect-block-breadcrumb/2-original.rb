RSpec.describe Client do
  it "raises on repeated failure" do
    setup_stub
    expect {
      collect_pages(max)
      finalize
    }.to raise_error(RetryError)
  end
end
