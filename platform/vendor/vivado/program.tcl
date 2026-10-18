# Load a bitstream over JTAG into the first device Vivado's hardware server
# finds (volatile: lost at power-off).
#
#   vivado -mode batch -nojournal -source platform/vendor/vivado/program.tcl \
#       -tclargs <bitstream>
#
# UNTESTED (docs/plans/platform-layer.md §6): whether the Mimas A7's FT2232H
# JTAG channel is visible to hw_server without Numato's configuration tool is
# the first thing to establish on a Vivado machine.

set bit [file normalize [lindex $argv 0]]
open_hw_manager
connect_hw_server
open_hw_target
set dev [lindex [get_hw_devices] 0]
current_hw_device $dev
set_property PROGRAM.FILE $bit $dev
program_hw_devices $dev
close_hw_manager
