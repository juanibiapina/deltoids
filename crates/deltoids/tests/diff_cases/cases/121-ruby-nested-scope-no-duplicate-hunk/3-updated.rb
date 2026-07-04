namespace :github do
  desc "Sync (MAX_PAGES=5)"
  task sync: :environment do
    max_pages = 5
    result = call(pages: max_pages, retries: 3)
    puts "done #{result} pages=#{max_pages}"
  end
end
