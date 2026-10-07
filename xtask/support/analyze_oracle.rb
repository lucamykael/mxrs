# frozen_string_literal: true
require_relative 'command_oracle_support'

# `analyze` finds what reads badly in the model's OQL, or in a query given as
# --sql/--oql: rule, severity, the fragment it is about, the message and each
# dialect's suggestion — the same text and JSON from both CLIs.
o = CommandOracle.new('analyze')
# Each CLI names itself in its no-findings line.
SAME = ->(native, ours) { [native.sub('[mxrb]', '[mxrs]'), ours] }

QUERIES = [
  "SELECT * FROM Sales.Order o, Sales.Line l WHERE lower(o/Name) LIKE '%a%'",
  "SELECT COUNT(*), o/* FROM Sales.Order AS o JOIN o/Sales.Line AS l WHERE o/Code LIKE 'x%'",
  "SELECT o/Name FROM Sales.Order o WHERE CAST(o/Total AS DECIMAL) > 1 AND o/Name LIKE '%it''s' ORDER BY o/Name",
  "SELECT DISTINCT\n  *\nFROM Sales.Order\nWHERE UPPER (Name) = 'A' GROUP BY Name",
  'SELECT Name FROM Sales.Order',
  'SELECT * WHERE x'
].freeze

def fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) do
      entity(:Order) { string :Name }
    end
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  parent = mpr.units_by_containment('Modules').find { mpr.parse_contents(_1)['Name'] == 'Sales' }['UnitID']
  QUERIES.first(3).each_with_index do |query, index|
    mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: {
      '$Type' => 'DomainModels$ViewEntitySourceDocument', 'Name' => "View#{index}", 'Oql' => query
    })
  end
  mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: {
    '$Type' => 'DataSets$DataSet', 'Name' => 'Report',
    'Source' => { '$Type' => 'DataSets$OqlDataSetSource', 'Query' => QUERIES[3] }
  })
  mpr.close
end

Dir.mktmpdir('mxrs-analyze-oracle-') do |root|
  path = File.join(root, 'Project.mpr')
  fixture(path)
  o.readonly(root) do
    [[], ['--json'], ['--dialect', 'sql_server'], ['--dialect', 'ansi', '--json']].each do |extra|
      o.equivalent(path, *extra, &SAME)
    end
  end
  QUERIES.each do |query|
    [['--oql', query], ['--sql', query], ['--oql', query, '--json'], ['--sql', query, '--dialect', 'sql_server']].each do |args|
      o.equivalent(*args, &SAME)
    end
  end
  o.failure('--sql', 'x', '--oql', 'y')
  o.failure(path, '--dialect', 'oracle')
end
o.finish
