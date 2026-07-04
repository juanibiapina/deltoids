RSpec.describe Thing do
  it "recovers the ghost author" do
    stub_request(:post, "https://example.test/graphql")
      .to_return(json_response(fixture("null_author.json")))
    stub_request(:get, "https://example.test/issues/2")
      .to_return(json_response(fixture("rest_ghost.json")))

    expect(result).to include(github_id: 10137, login: "ghost")
  end

  it "raises on a userless success" do
    stub_request(:post, "https://example.test/graphql")
      .to_return(json_response(fixture("null_author.json")))
    stub_request(:get, "https://example.test/issues/2")
      .to_return(json_response({ "number" => 2, "user" => nil }.to_json))

    expect { collect }.to raise_error(RecoveryError)
  end
end
