RSpec.describe Thing do
  it "old name here" do
    stub_request(:post, "https://example.test/graphql")
      .to_return(json_response(fixture("null_author.json")))
    stub_request(:get, "https://example.test/issues/2")
      .to_return(json_response({ "number" => 2, "user" => nil }.to_json))

    expect(result).to include(login: "ghost")
  end
end
