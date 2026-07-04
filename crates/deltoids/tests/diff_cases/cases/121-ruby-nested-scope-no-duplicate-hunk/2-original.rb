namespace :github do
  desc "Sync (MAX_PAGES=1)"
  task sync: :environment do
    max_pages = 1
    result = call(pages: max_pages)
    puts "done #{result}"
  end
end
