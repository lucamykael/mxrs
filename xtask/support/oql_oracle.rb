# frozen_string_literal: true
require_relative 'command_oracle_support'

# `oql` lists the model's OQL and its logical SQL. Both CLIs must print the
# same text, warnings and JSON, and exit alike.
o = CommandOracle.new('oql')

def same(o, *args)
  native = o.run(true, *args).map { _1.is_a?(String) ? _1.gsub('[mxrb]', '[mxrs]') : _1 }
  ours = o.run(false, *args)
  o.check("oql mismatch for #{args.inspect}: #{native.inspect} != #{ours.inspect}") { native == ours }
end

QUERIES = [
  'SELECT o/Number AS Number, o/Total FROM Sales.Order AS o WHERE o/Total > $min ORDER BY o/Number',
  'FROM Sales.Order AS o SELECT o/Number',
  'SELECT COUNT(*) AS Total FROM Sales.Order',
  'SELECT l/Quantity FROM Sales.Order AS o INNER JOIN o/Sales.Line_Order/Sales.Line AS l',
  "SELECT o/Number FROM Sales.Order AS o WHERE o/Name LIKE '%x%'"
].freeze

def define(path, queries)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) { entity(:Order) { string :Number } }
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  parent = mpr.units_by_containment('Modules').find { mpr.parse_contents(_1)['Name'] == 'Sales' }['UnitID']
  queries.each_with_index do |query, index|
    mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: {
      '$Type' => 'DomainModels$ViewEntitySourceDocument', 'Name' => "View#{index}", 'Oql' => query
    })
  end
  mpr.close
end

Dir.mktmpdir('mxrs-oql-oracle-') do |root|
  path = File.join(root, 'with/Project.mpr')
  define(path, QUERIES)
  empty = File.join(root, 'empty/Project.mpr')
  define(empty, [])
  o.readonly(root) do
    [[], ['--json'], ['--dialect', 'sql_server'], ['--dialect', 'ansi', '--json']].each do |extra|
      same(o, path, *extra)
      same(o, empty, *extra)
    end
  end
  o.failure
  o.failure(path, '--dialect', 'oracle')
end
o.finish
