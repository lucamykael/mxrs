# frozen_string_literal: true
require_relative 'command_oracle_support'

# `search` ranks every artifact by the cosine distance of its hashed
# term-frequency vector to the query's: rank, distance to six places,
# qualified name and kind — or the same as JSON. Both run the TF-IDF backend.
o = CommandOracle.new('search')

def fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) do
      entity(:Order) { string :Number; string :Status }
      entity(:Customer) { string :Name }
      microflow(:ACT_Order_Save) {}
      microflow(:ACT_Order_Delete) {}
      microflow(:SUB_Customer_Notify) {}
      nanoflow(:NAN_Order_Open) {}
      page(:Order_Overview) { title 'Orders' }
    end
    self.module(:Billing) do
      entity(:Invoice) { string :Total }
      microflow(:ACT_Invoice_Pay) {}
    end
  end
end

Dir.mktmpdir('mxrs-search-oracle-') do |root|
  path = File.join(root, 'Project.mpr')
  fixture(path)
  o.readonly(root) do
    ['order', 'save order', 'customer notify microflow', 'Billing.Invoice', 'pay invoice total', 'zz9'].each do |text|
      [[], ['--limit', '3'], ['--json'], ['--limit', '100', '--json']].each do |extra|
        o.equivalent(text, path, '--backend', 'tfidf', *extra)
      end
    end
  end
  o.failure('order', path, '--limit', '0')
  o.failure('order', File.join(root, 'Missing.mpr'), '--backend', 'tfidf')
end
o.finish
