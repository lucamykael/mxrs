# frozen_string_literal: true
require_relative 'command_oracle_support'

# `query` converts a read-only query between SQL and logical OQL. Both CLIs
# must print the same text, warnings and JSON, and exit alike.
o = CommandOracle.new('query')

def same(o, *args)
  native = o.run(true, *args).map { _1.is_a?(String) ? _1.gsub('[mxrb]', '[mxrs]') : _1 }
  ours = o.run(false, *args)
  o.check("query mismatch for #{args.inspect}: #{native.inspect} != #{ours.inspect}") { native == ours }
end

SQL = [
  'SELECT o.Number, COUNT(*) FROM public.sales$order o WHERE o.Total / 2 <> :limit AND o.Name = @name',
  'select o.number, o.totalamount from sales$order as o where o.number like :prefix',
  'SELECT * FROM Sales.Order JOIN Sales.Line l ON l.Order = Order.ID ORDER BY Order.Number',
  'SELECT LEN(Name), CHAR_LENGTH(Name) FROM "Sales"."Order"',
  'SELECT [Number] FROM dbo.[sales$order] WHERE [Number] = $p',
  'DELETE FROM Sales.Order',
  'WITH x AS (SELECT 1) SELECT * FROM x',
  'SELECT * FROM Sales.Order; SELECT 1',
  'SELECT x::int FROM Sales.Order',
  'SELECT * FROM Sales.Order WHERE Name ILIKE :n',
  'SELECT DISTINCT ON (Name) Name FROM Sales.Order',
  'SELECT * FROM (SELECT 1) t',
  'SELECT md5(Name) FROM Sales.Order',
  "SELECT Name || 'x' FROM Sales.Order",
  'SELECT * FROM Sales.Order WHERE Id = ?',
  'SELECT * FROM Sales.Order WHERE Id = $1',
  'SELECT * FROM orders',
  'SELECT Name FROM Sales.Order WHERE Name = :'
].freeze

OQL = [
  'SELECT o/Number FROM Sales.Order AS o WHERE o/Total > $min',
  'FROM Sales.Order AS o SELECT o/Number',
  'SELECT * FROM Sales.Order',
  'DELETE FROM Sales.Order'
].freeze

def define(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) do
      entity(:Order) { string :Number; decimal :TotalAmount }
    end
  end
end

Dir.mktmpdir('mxrs-query-oracle-') do |root|
  project = File.join(root, 'Project.mpr')
  define(project)
  SQL.each do |sql|
    same(o, sql, '--from', 'sql')
    same(o, sql, '--from', 'sql', '--json')
    same(o, sql, '--from', 'sql', '--to', 'oql', '--dialect', 'sql_server', '--json')
    o.readonly(root) { same(o, sql, '--from', 'sql', '--project', project, '--json') }
  end
  OQL.each do |oql|
    same(o, oql, '--from', 'oql')
    same(o, oql, '--from', 'oql', '--json')
    same(o, oql, '--from', 'oql', '--dialect', 'ansi', '--json')
  end
  input = File.join(root, 'query.sql')
  File.write(input, SQL.first)
  same(o, '--from', 'sql', '--input', input, '--json')
  o.failure('SELECT 1')
  o.failure('SELECT 1', '--from', 'xml')
  o.failure('SELECT 1', '--from', 'sql', '--to', 'sql')
  o.failure('SELECT 1', '--from', 'oql', '--project', project)
end
o.finish
