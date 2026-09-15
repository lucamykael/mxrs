# frozen_string_literal: true

require 'mxrb'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'digest'
require 'rbconfig'
require 'json'

# Shared harness, never shared expected results: every command supplies its
# native fixtures, output comparison, and explicit normalization contract.
class CommandOracle
  attr_reader :checks, :mxrs, :mxrb

  def initialize(command, surface: command)
    @command, @surface = command, surface
    @mxrs = File.expand_path(ARGV.fetch(0))
    @mxrb = File.expand_path('bin/mxrb')
    raise 'run via xtask from the MXRB checkout' unless File.file?(@mxrb)
    raise 'MXRS executable missing' unless File.executable?(@mxrs)
    @checks = 0
  end

  def run(native, *args, **options)
    program = native ? [RbConfig.ruby, mxrb, @command] : [mxrs, @surface]
    out, err, status = Open3.capture3(*program, *args, **options)
    [out, err, status.exitstatus]
  end

  def success(native, *args, **options)
    out, err, status = run(native, *args, **options)
    raise "#{@command} failed (native=#{native}): #{err}" unless status == 0 && err.empty?
    out
  end

  def check(label)
    raise "#{@command}: #{label}" unless yield
    @checks += 1
  end

  def equivalent(*args, **options)
    lhs = success(true, *args, **options)
    rhs = success(false, *args, **options)
    lhs, rhs = yield(lhs, rhs) if block_given?
    check("output mismatch for #{args.inspect}: #{lhs.inspect} != #{rhs.inspect}") { lhs == rhs }
  end

  def failure(*args, native: true)
    (native ? [true, false] : [false]).each do |implementation|
      out, err, code = run(implementation, *args)
      # Ruby may emit a partial report before encountering malformed content.
      check("invalid input accepted: #{args.inspect}") { code && code != 0 && !err.empty? && (implementation || out.empty?) }
    end
  end

  def help
    %w[--help -h].each do |flag|
      [true, false].each do |native|
        check('help') { success(native, flag).include?(native ? @command : @surface) }
      end
    end
  end

  def files(root)
    Dir.glob(File.join(root, '**', '*'), File::FNM_DOTMATCH).select { File.file?(_1) }
       .to_h { [_1.delete_prefix("#{root}/"), Digest::SHA256.file(_1).hexdigest] }
  end

  def readonly(root)
    before = files(root)
    yield
    check('source files or inventory changed') { files(root) == before }
  end

  def finish
    puts "PASS #{@command} command oracle: #{checks} checks; both CLIs executed; temporary fixtures removed"
  end
end

def oracle_mpr(path, format: :v1)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) { entity(:Record) { string :Name } }
    self.module(:Alpha) {}
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  mpr.ensure_storage_for_version!('11.12.1') if format == :v2
  raise 'wrong fixture storage format' unless mpr.format_version == format
  yield mpr if block_given?
ensure
  mpr&.close
end
