# frozen_string_literal: true
require_relative 'command_oracle_support'
require 'zip'

def mda_fixture(path, entries)
  Zip::OutputStream.open(path) do |zip|
    entries.each do |name, bytes|
      zip.put_next_entry(name.start_with?('/') ? "x#{name[1..]}" : name)
      zip.write(bytes) unless name.end_with?('/')
    end
  end
  entries.keys.select { _1.start_with?('/') }.each do |name|
    bytes = File.binread(path).gsub("x#{name[1..]}", name)
    File.binwrite(path, bytes)
  end
end

o = CommandOracle.new('mda')
Dir.mktmpdir('mxrs-mda-oracle-') do |root|
  metadata = JSON.generate('RuntimeVersion' => '11.12.1', 'ProjectName' => 'Projeto Á', 'Extra' => [1, true, nil])
  entries = { 'model/' => '', 'empty/' => '', 'model/metadata.json' => metadata,
              'model/old.bin' => "\x00\xff".b, 'web/index.html' => 'left', 'web/á.txt' => '' }
  left = File.join(root, 'left with spaces.mda')
  right = File.join(root, 'right.mda')
  reordered = File.join(root, 'reordered.mda')
  mda_fixture(left, entries)
  mda_fixture(reordered, entries.to_a.reverse.to_h)
  mda_fixture(right, entries.reject { |key, _| key == 'model/old.bin' }.merge('model/new.bin' => 'new', 'web/index.html' => 'right'))
  empty_metadata = File.join(root, 'empty.mda')
  mda_fixture(empty_metadata, 'model/metadata.json' => '{}')
  o.readonly(root) do
    [left, right, reordered, empty_metadata].each do |path|
      o.equivalent('inspect', path)
      o.equivalent('inspect', path, '--no-progress')
      o.equivalent('inspect', path, '--json') { |a, b| [JSON.parse(a), JSON.parse(b)] }
    end
    [[left, left], [left, reordered], [left, right], [right, left]].each do |a, b|
      o.equivalent('compare', a, b) { |native, rust| [native.sub('[mxrb]', '[mxrs]'), rust] }
    end
  end
  { 'missing' => { 'web/file' => '' }, 'json' => { 'model/metadata.json' => '{' },
    'traversal' => { 'model/metadata.json' => metadata, '../bad' => '' },
    'dot' => { 'model/metadata.json' => metadata, 'web/./bad' => '' },
    'null' => { 'model/metadata.json' => 'null' } }.each do |name, contents|
    path = File.join(root, "bad-#{name}.mda")
    mda_fixture(path, contents)
    o.readonly(root) do
      o.failure('inspect', path)
      o.failure('compare', left, path)
    end
  end
  # Rust additionally refuses ambiguous/absolute names and non-object metadata.
  { 'absolute' => { 'model/metadata.json' => metadata, '/bad' => '' },
    'doubled' => { 'model/metadata.json' => metadata, 'web//bad' => '' },
    'array' => { 'model/metadata.json' => '[]' },
    'typed' => { 'model/metadata.json' => '{"RuntimeVersion":42}' } }.each do |name, contents|
    path = File.join(root, "strict-#{name}.mda")
    mda_fixture(path, contents)
    o.readonly(root) { o.failure('inspect', path, native: false) }
  end
  duplicate = File.join(root, 'duplicate.mda')
  mda_fixture(duplicate, 'model/metadata.json' => metadata, 'web/file' => 'one', 'web/filx' => 'two')
  File.binwrite(duplicate, File.binread(duplicate).gsub('web/filx', 'web/file'))
  o.readonly(root) { o.failure('inspect', duplicate, native: false) }
  corrupt = File.join(root, 'corrupt.mda')
  File.binwrite(corrupt, 'not ZIP')
  [[], ['unknown'], ['inspect'], ['compare', left], ['inspect', corrupt], ['inspect', root],
   ['inspect', File.join(root, 'absent.mda')]].each { o.readonly(root) { o.failure(*_1) } }
  [['inspect', left, '--json', '--json'], ['inspect', left, '--unknown'], ['inspect', left, 'extra'],
   ['compare', left, right, 'extra'], ['compare', left, right, '--json']].each { o.failure(*_1, native: false) }
  o.help
end
o.finish
