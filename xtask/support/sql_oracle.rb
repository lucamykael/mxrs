# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new('sql')
# The native CLI prints Ruby inspect strings. Normalize only presentation:
# JSON-quoted UTF-8 text, NULL, lossless typed hex bytes, numeric values.
def sql_cells(rows)
  rows.map do |row|
    row.map do |value|
      case value
      when nil then 'NULL'
      when Integer then value.to_s
      when Float then value
      when String
        if value.encoding == Encoding::ASCII_8BIT
          "X'#{value.unpack1('H*')}'"
        elsif !value.valid_encoding?
          "TEXT X'#{value.unpack1('H*')}'"
        else
          JSON.generate(value)
        end
      else raise "unknown native SQL value #{value.class}"
      end
    end
  end
end

Dir.mktmpdir('mxrs-sql-oracle-') do |root|
  %i[v1 v2].each do |format|
    path = File.join(root, "#{format}/Project.mpr")
    oracle_mpr(path, format:)
    queries = [
      'SELECT COUNT(*) FROM Unit',
      'SELECT UnitID, ContainerID, ContainmentName, ContentsHash FROM Unit ORDER BY rowid',
      'SELECT NULL, -9223372036854775808, 9223372036854775807, 1.0, -0.25, 1e100, 1e-100, 1e999',
      "SELECT 'Olá \"SQL\"', char(0, 7, 8, 9, 10, 11, 12, 13, 27, 127), '\\#{'#{safe}'}'",
      "SELECT X'', X'00017fff80c3a9', CAST(X'80ff' AS TEXT), ''",
      'SELECT * FROM Unit WHERE 0',
      'WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<4) SELECT x FROM n',
      'PRAGMA table_info(Unit)',
      'PRAGMA database_list',
      'PRAGMA integrity_check',
      'SELECT ?'
    ]
    mpr = Mxrb::IO::MprFile.open(path, readonly: true)
    o.readonly(root) do
      queries.each do |query|
        rows = mpr.query(query)
        native = o.success(true, path, query)
        o.check('native CLI differs from native query values') { native == rows.map { "#{_1.inspect}\n" }.join + "(#{rows.size} rows)\n" }
        expected = sql_cells(rows)
        [[], ['--no-progress']].each do |flags|
          output = o.success(false, path, query, *flags)
          lines = output.lines
          o.check('row count') { lines.pop == "(#{rows.size} rows)\n" }
          # Split with the expected string boundaries; JSON strings may contain
          # commas. Floats alone admit equivalent exponent notation.
          pattern = expected.map do |row|
            '\\[' + row.map { _1.is_a?(Float) ? '([^,\\]]+)' : Regexp.escape(_1) }.join(', ') + "\\]\\n"
          end.join
          match = Regexp.new("\\A#{pattern}\\z").match(lines.join)
          o.check('all cells, types, order and bytes') do
            match && match.captures.zip(expected.flatten.grep(Float)).all? do |actual, value|
              parsed = actual == 'inf' ? Float::INFINITY : actual == '-inf' ? -Float::INFINITY : Float(actual)
              parsed == value
            end
          end
        end
      end
      ['-- empty query', '', 'SELECT broken FROM Unit', 'not SQL', 'DELETE FROM Unit', 'CREATE TABLE forbidden(x)'].each { o.failure(path, _1) }
      ['SELECT 1; SELECT 2', "ATTACH DATABASE '#{root}/outside.sqlite' AS outside", "VACUUM INTO '#{root}/copy.sqlite'",
       'CREATE TEMP TABLE forbidden(x)', 'PRAGMA user_version=42', "SELECT load_extension('missing')"].each do |query|
        o.failure(path, query, native: false)
      end
    end
    mpr.close
    [%w[--unknown], %w[extra], %w[--no-progress --no-progress]].each { o.failure(path, 'SELECT 1', *_1, native: false) }
  end
  corrupt = File.join(root, 'corrupt.mpr')
  File.write(corrupt, 'invalid')
  o.readonly(root) do
    [[], [File.join(root, 'absent.mpr'), 'SELECT 1'], [corrupt, 'SELECT 1'], [root, 'SELECT 1']].each { o.failure(*_1) }
  end
  o.help
end
o.finish
