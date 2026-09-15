# frozen_string_literal: true

# Runs both real CLIs against independently generated, disposable native models.
# The MXRB checkout is read-only. No private fixture or network access is needed.
require 'mxrb'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'digest'
require 'rbconfig'

mxrs = File.expand_path(ARGV.fetch(0))
mxrb = File.expand_path('bin/mxrb')
raise 'MXRS executable not found' unless File.executable?(mxrs)
raise 'run through xtask with the MXRB checkout as cwd' unless File.file?(mxrb)

def run(*args)
  stdout, stderr, status = Open3.capture3(*args)
  [stdout, stderr, status.exitstatus]
end

def files(root)
  Dir.glob(File.join(root, '**', '*'), File::FNM_DOTMATCH).select { File.file?(_1) }
     .to_h { [_1.delete_prefix("#{root}/"), Digest::SHA256.file(_1).hexdigest] }
end

def native_records(output)
  output.lines.map do |line|
    match = /\A#<Mxrb::Module name=(.+) entities=(\d+) pages=(\d+) microflows=(\d+)>\n?\z/.match(line)
    raise "unexpected native output: #{line.inspect}" unless match

    { 'name' => match[1] == 'nil' ? nil : JSON.parse(match[1]),
      'entities' => match[2].to_i, 'pages' => match[3].to_i, 'microflows' => match[4].to_i }
  end
end

def readable_records(output)
  output.lines.map do |line|
    match = /\A(.+): entities=(\d+) pages=(\d+) microflows=(\d+)\n?\z/.match(line)
    raise "unexpected MXRS output: #{line.inspect}" unless match

    { 'name' => JSON.parse(match[1]), 'entities' => match[2].to_i,
      'pages' => match[3].to_i, 'microflows' => match[4].to_i }
  end
end

def native_fixture(path, empty: false)
  Mxrb.define(path) do
    mendix_version '11.12.1'
    unless empty
      self.module(:Zulu) do
        entity(:Record) { string :Name }
        entity(:Other) {}
        page(:Direct) { title 'Direct' }
        page(:Nested) { title 'Nested' }
        microflow(:Direct) {}
        microflow(:Nested) {}
        nanoflow(:Client) {}
      end
      self.module(:Alpha) {}
      self.module(:Unnamed) {}
    end
  end
  return if empty

  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  modules = mpr.units_by_containment('Modules')
  zulu = modules.find { mpr.parse_contents(_1)['Name'] == 'Zulu' }.fetch('UnitID')
  alpha = modules.find { mpr.parse_contents(_1)['Name'] == 'Alpha' }
  alpha_doc = mpr.parse_contents(alpha)
  alpha_doc['Name'] = "Álpha \"quoted\""
  mpr.update_unit(alpha.fetch('UnitID'), alpha_doc)
  unnamed = modules.find { mpr.parse_contents(_1)['Name'] == 'Unnamed' }
  unnamed_doc = mpr.parse_contents(unnamed)
  unnamed_doc.delete('Name')
  mpr.update_unit(unnamed.fetch('UnitID'), unnamed_doc)
  # A missing domain model is a legitimate empty-module representation.
  mpr.units_by_containment('DomainModel').select { _1['ContainerID'] == unnamed['UnitID'] }
     .each { mpr.delete_unit(_1.fetch('UnitID')) }
  folder = mpr.insert_unit(container_uuid: zulu, containment_name: 'Folders',
                           contents_doc: { '$Type' => 'Projects$Folder', 'Name' => 'Outer' })
  inner = mpr.insert_unit(container_uuid: folder, containment_name: 'Folders',
                          contents_doc: { '$Type' => 'Projects$Folder', 'Name' => 'Inner' })
  mpr.units_by_containment('Documents').select { _1['ContainerID'] == zulu }.each do |unit|
    doc = mpr.parse_contents(unit)
    next unless doc['Name'] == 'Nested'

    if doc['$Type'] == 'Forms$Page'
      doc['$Type'] = 'Pages$Page'
      mpr.update_unit(unit.fetch('UnitID'), doc)
    end
    mpr.relocate_unit(unit.fetch('UnitID'), container_uuid: inner, containment_name: 'Documents')
  end
  mpr.insert_unit(container_uuid: inner, containment_name: 'Documents',
                  contents_doc: { '$Type' => 'Microflows$Rule', 'Name' => 'Rule' })
  mpr.insert_unit(container_uuid: inner, containment_name: 'Documents',
                  contents_doc: { '$Type' => 'Future$Document', 'Name' => 'Future' })
ensure
  mpr&.close
end

checks = 0
Dir.mktmpdir('mxrs-modules-oracle-') do |root|
  fixtures = %w[empty v1 v2].map do |name|
    directory = File.join(root, name)
    FileUtils.mkdir_p(directory)
    path = File.join(directory, 'Project with spaces.mpr')
    native_fixture(path, empty: name == 'empty')
    if name == 'v2'
      mpr = Mxrb::IO::MprFile.open(path, readonly: false)
      mpr.ensure_storage_for_version!('11.12.1')
      mpr.close
    end
    mpr = Mxrb::IO::MprFile.open(path, readonly: true)
    raise 'fixture storage version mismatch' unless mpr.format_version == (name == 'v2' ? :v2 : :v1)
    mpr.close
    [name, path]
  end
  fixtures.each do |name, path|
    before = files(root)
    native, errors, status = run(RbConfig.ruby, mxrb, 'modules', path)
    raise "native #{name} failed: #{errors}" unless status.zero? && errors.empty?

    expected = native_records(native)
    if name != 'empty'
      raise 'fixture does not exercise nested documents and exclusions' unless expected.any? do |record|
        record == { 'name' => 'Zulu', 'entities' => 2, 'pages' => 2, 'microflows' => 2 }
      end
      raise 'unnamed module disappeared' unless expected.any? { _1['name'].nil? }
    end
    [[], ['--no-progress']].each do |flags|
      out, err, code = run(mxrs, 'modules', path, *flags)
      raise "MXRS #{name} failed: #{err}" unless code.zero? && err.empty?
      raise "default records differ for #{name}" unless readable_records(out) == expected

      checks += 1
    end
    out, err, code = run(mxrs, 'modules', path, '--json')
    raise "JSON differs for #{name}: #{err}" unless code.zero? && err.empty? && JSON.parse(out) == expected

    checks += 1
    out, err, code = run(mxrs, 'modules', path, '--names')
    names = expected.filter_map { _1['name'] }.sort.map { "#{_1}\n" }.join
    raise "names compatibility failed for #{name}: #{err}" unless code.zero? && err.empty? && out == names

    checks += 1
    raise "read-only command changed #{name}" unless files(root) == before

    puts "PASS modules #{name}: records, order, names, JSON and read-only storage"
  end
  corrupt = File.join(root, 'corrupt.mpr')
  File.binwrite(corrupt, 'not a SQLite database')
  [[], [File.join(root, 'missing.mpr')], [corrupt], [root]].each do |arguments|
    before = files(root)
    [[RbConfig.ruby, mxrb], [mxrs]].each do |program|
      out, err, code = run(*program, 'modules', *arguments)
      raise "invalid input accepted: #{arguments.inspect}" unless code && !code.zero? && out.empty? && !err.empty?
    end
    raise 'invalid input changed storage' unless files(root) == before

    checks += 1
  end
  [['--help'], ['-h']].each do |flags|
    [[RbConfig.ruby, mxrb], [mxrs]].each do |program|
      out, err, code = run(*program, 'modules', *flags)
      raise 'help contract failed' unless code.zero? && err.empty? && out.include?('modules')
    end
    checks += 1
  end
  valid = fixtures[1].last
  [['--unknown'], ['--json', '--names'], ['extra'], ['--json', '--json']].each do |flags|
    out, err, code = run(mxrs, 'modules', valid, *flags)
    raise 'MXRS must reject ambiguous arguments' unless code && !code.zero? && out.empty? && !err.empty?

    checks += 1
  end
end
puts "PASS modules command oracle: #{checks} checks; both CLIs executed; temporary models removed"
