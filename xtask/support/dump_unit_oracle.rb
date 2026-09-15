# frozen_string_literal: true

# Native fixtures and both real CLIs; successful stdout must match byte for byte.
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
  out, err, status = Open3.capture3(*args)
  [out, err, status.exitstatus]
end

def files(root)
  Dir.glob(File.join(root, '**', '*'), File::FNM_DOTMATCH).select { File.file?(_1) }
     .to_h { [_1.delete_prefix("#{root}/"), Digest::SHA256.file(_1).hexdigest] }
end

def fixture(path, version)
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Diagnostics) { entity(:Record) { string :Name } }
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  parent = mpr.units_by_containment('Modules').first.fetch('UnitID')
  ids = {}
  documents = {
    'binary' => { '$Type' => 'Future$Diagnostic', 'Name' => "á\nquoted \"name\"",
                  'Payload' => BSON::Binary.new((0..255).to_a.pack('C*') * 260, :generic) },
    'untyped' => { 'Name' => 'Without a type tag' },
    'empty' => { '$Type' => 'Future$Empty' },
    'absent' => { '$Type' => 'Future$Absent' },
    'no-hash' => { '$Type' => 'Future$NoHash' },
    'corrupt' => { '$Type' => 'Future$Corrupt' }
  }
  documents.each do |name, doc|
    ids[name] = mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: doc)
  end
  ids['module'] = parent
  mpr.ensure_storage_for_version!('11.12.1') if version == :v2
  raise 'fixture storage version mismatch' unless mpr.format_version == version

  { 'empty' => ''.b, 'absent' => nil, 'corrupt' => 'broken BSON'.b }.each do |name, bytes|
    unit = mpr.unit(ids.fetch(name))
    if version == :v2
      file = mpr.content_path(unit)
      bytes.nil? ? File.unlink(file) : File.binwrite(file, bytes)
    else
      mpr.query('UPDATE Unit SET Contents = ? WHERE UnitID = ?',
                [bytes && SQLite3::Blob.new(bytes), Mxrb::IO::BsonCodec.uuid_to_blob(ids.fetch(name))])
    end
  end
  mpr.query('UPDATE Unit SET ContentsHash = NULL WHERE UnitID = ?',
            [Mxrb::IO::BsonCodec.uuid_to_blob(ids.fetch('no-hash'))])
  ids
ensure
  mpr&.close
end

checks = 0
Dir.mktmpdir('mxrs-dump-oracle-') do |root|
  projects = %i[v1 v2].map do |version|
    directory = File.join(root, version.to_s)
    FileUtils.mkdir_p(directory)
    path = File.join(directory, 'Project with spaces.mpr')
    [path, fixture(path, version)]
  end
  projects.each do |path, ids|
    before = files(root)
    ids.reject { |name, _id| name == 'corrupt' }.each do |name, id|
      [[], ['--no-progress']].each do |flags|
        native, native_error, native_status = run(RbConfig.ruby, mxrb, 'dump-unit', path, id, *flags)
        rust, rust_error, rust_status = run(mxrs, 'dump-unit', path, id, *flags)
        raise "native #{name} failed: #{native_error}" unless native_status.zero? && native_error.empty?
        raise "MXRS #{name} failed: #{rust_error}" unless rust_status.zero? && rust_error.empty?
        raise "dump differs for #{name}" unless native == rust
        raise 'large offsets not covered' if name == 'binary' && !rust.include?('  10000  ')
        raise 'empty byte sequence conflated with absent bytes' if name == 'empty' && !rust.end_with?("Contents (hex)   :\n")
        raise 'missing bytes marker absent' if name == 'absent' && !rust.end_with?("  (empty)\n")

        checks += 1
      end
    end
    id = ids.fetch('module')
    [id.upcase, id.delete('-')].each do |identifier|
      native, ne, ns = run(RbConfig.ruby, mxrb, 'dump-unit', path, identifier)
      rust, re, rs = run(mxrs, 'dump-unit', path, identifier)
      raise 'UUID lookup mismatch' unless ns.zero? && rs.zero? && ne.empty? && re.empty? && native == rust

      checks += 1
    end
    ['00000000-0000-0000-0000-000000000000', 'not-an-id', ids.fetch('corrupt')].each do |identifier|
      _native, ne, ns = run(RbConfig.ruby, mxrb, 'dump-unit', path, identifier)
      rust, re, rs = run(mxrs, 'dump-unit', path, identifier)
      # MXRB prints identity headers before discovering corrupt BSON. MXRS
      # validates first, so failure never presents a partial successful dump.
      raise 'invalid unit did not fail' unless ns && rs && !ns.zero? && !rs.zero? && !ne.empty? && !re.empty? && rust.empty?

      checks += 1
    end
    raise 'dump-unit modified project files' unless files(root) == before

    puts "PASS dump-unit #{File.basename(File.dirname(path))}: exact metadata/hex/ASCII, empty/absent bytes, UUID lookup, corrupt BSON and read-only storage"
  end
  path, ids = projects.first
  id = ids.fetch('module')
  corrupt = File.join(root, 'corrupt.mpr')
  File.binwrite(corrupt, 'not SQLite')
  [[], [path], [File.join(root, 'missing.mpr'), id], [corrupt, id], [root, id]].each do |arguments|
    before = files(root)
    [[RbConfig.ruby, mxrb], [mxrs]].each do |program|
      out, err, code = run(*program, 'dump-unit', *arguments)
      raise 'invalid request accepted' unless code && !code.zero? && out.empty? && !err.empty?
    end
    raise 'invalid request modified files' unless files(root) == before

    checks += 1
  end
  [['--help'], ['-h']].each do |flags|
    [[RbConfig.ruby, mxrb], [mxrs]].each do |program|
      out, err, code = run(*program, 'dump-unit', *flags)
      raise 'help failed' unless code.zero? && err.empty? && out.include?('dump-unit')
    end
    checks += 1
  end
  [['extra'], ['--unknown'], ['--no-progress', '--no-progress']].each do |flags|
    out, err, code = run(mxrs, 'dump-unit', path, id, *flags)
    raise 'ambiguous arguments accepted' unless code && !code.zero? && out.empty? && !err.empty?

    checks += 1
  end
end
puts "PASS dump-unit command oracle: #{checks} checks; exact successful stdout; temporary models removed"
