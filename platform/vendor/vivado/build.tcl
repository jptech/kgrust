# Vivado non-project flow for a bundle written by `kg board bundle`
# (or `kg board bitstream`): synthesis, implementation, reports, bitstream.
#
#   vivado -mode batch -nojournal -source platform/vendor/vivado/build.tcl \
#       -tclargs <bundle dir>
#
# The bundle's board.tcl sets `part` and `top`; sources.f and constraints.f
# list one absolute path per line, in compile order (upstream's runtime
# packages first, as its files.f requires).
#
# UNTESTED: written without Vivado to hand (docs/plans/platform-layer.md §6).

if {[llength $argv] != 1} {
    puts "usage: vivado -mode batch -source build.tcl -tclargs <bundle dir>"
    exit 2
}
set bundle [file normalize [lindex $argv 0]]
source [file join $bundle board.tcl]

proc read_list {path} {
    set f [open $path r]
    set paths {}
    foreach line [split [read $f] "\n"] {
        set line [string trim $line]
        if {$line ne "" && [string index $line 0] ne "#"} { lappend paths $line }
    }
    close $f
    return $paths
}

set out [file join $bundle vivado]
file mkdir $out
cd [file join $bundle gen]

read_verilog -sv [read_list [file join $bundle sources.f]]
read_xdc [read_list [file join $bundle constraints.f]]
# Initialised memories: the backend passes each .mem file's bare name to the
# HAL as INITIAL_DATA_FILE (compiler/cpp/verilog.cpp:2645), which becomes
# xpm_memory's MEMORY_INIT_FILE (hal/xilinx/hal_dual_port_ram.sv:156); in
# non-project mode Vivado finds it only among the design's files.
foreach mem [glob -nocomplain *.mem] { read_mem $mem }

synth_design -top $top -part $part
write_checkpoint -force [file join $out post_synth.dcp]
report_utilization -file [file join $out utilization_synth.rpt]

opt_design
place_design
phys_opt_design
route_design
write_checkpoint -force [file join $out post_route.dcp]

report_utilization -file [file join $out utilization.rpt]
report_timing_summary -max_paths 20 -file [file join $out timing_summary.rpt]
report_drc -file [file join $out drc.rpt]

write_bitstream -force [file join $out $top.bit]
puts "bitstream: [file join $out $top.bit]"
