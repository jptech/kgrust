# Numato Lab Mimas A7 (XC7A50T-1FGG484C): constraints for the board shells
# under platform/boards/mimas-a7.
#
# Pins and I/O standards are Numato's, from their "Mimas A7 V3.0 Constraints
# File" (numato.com/download/mimas-a7-v3-0-constraints-file/); every bank
# used here is 3.3 V. Only the ports a shell drives are live: Vivado raises a
# critical warning for a constraint whose port the design lacks. The rest of
# the board's simple I/O is listed commented out, ready to enable; DDR3, HDMI,
# Ethernet, DisplayPort and the headers are in Numato's file.
#
# UNVERIFIED on hardware (docs/plans/platform-layer.md §6): check the board
# revision (V3 assumed; V4.0 moved at least PROG_B) before the first bitstream.

set_property CFGBVS VCCO [current_design]
set_property CONFIG_VOLTAGE 3.3 [current_design]

# Configuration from the on-board quad-SPI flash; harmless for JTAG loading.
set_property BITSTREAM.CONFIG.SPI_BUSWIDTH 4 [current_design]
set_property BITSTREAM.GENERAL.COMPRESS TRUE [current_design]

## Clock: 100 MHz oscillator (ASEM1-100.000MHZ-LC-T) on a clock-capable pin
set_property -dict { PACKAGE_PIN H4  IOSTANDARD LVCMOS33 } [get_ports { CLK1 }]   ; # IO_L12P_T1_MRCC_35
create_clock -name sys_clk -period 10.000 [get_ports { CLK1 }]

## Reset: push button S3, active high
set_property -dict { PACKAGE_PIN M2  IOSTANDARD LVCMOS33 } [get_ports { RESET }]  ; # IO_L16N_T2_35

## LEDs LED0..LED7, active high
set_property -dict { PACKAGE_PIN K17 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[0] }] ; # IO_L21P_T3_DQS_15
set_property -dict { PACKAGE_PIN J17 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[1] }] ; # IO_L21N_T3_DQS_A18_15
set_property -dict { PACKAGE_PIN L14 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[2] }] ; # IO_L22P_T3_A17_15
set_property -dict { PACKAGE_PIN L15 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[3] }] ; # IO_L22N_T3_A16_15
set_property -dict { PACKAGE_PIN L16 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[4] }] ; # IO_L23P_T3_FOE_B_15
set_property -dict { PACKAGE_PIN K16 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[5] }] ; # IO_L23N_T3_FWE_B_15
set_property -dict { PACKAGE_PIN M15 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[6] }] ; # IO_L24P_T3_RS1_15
set_property -dict { PACKAGE_PIN M16 IOSTANDARD LVCMOS33 SLEW SLOW } [get_ports { LED[7] }] ; # IO_L24N_T3_RS0_15

## DIP switch DP0..DP7
set_property -dict { PACKAGE_PIN B21 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[0] }] ; # IO_L21P_T3_DQS_16
set_property -dict { PACKAGE_PIN A21 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[1] }] ; # IO_L21N_T3_DQS_16
set_property -dict { PACKAGE_PIN E22 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[2] }] ; # IO_L22P_T3_16
set_property -dict { PACKAGE_PIN D22 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[3] }] ; # IO_L22N_T3_16
set_property -dict { PACKAGE_PIN E21 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[4] }] ; # IO_L23P_T3_16
set_property -dict { PACKAGE_PIN D21 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[5] }] ; # IO_L23N_T3_16
set_property -dict { PACKAGE_PIN G21 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[6] }] ; # IO_L24P_T3_16
set_property -dict { PACKAGE_PIN G22 IOSTANDARD LVCMOS33 } [get_ports { dip_sw[7] }] ; # IO_L24N_T3_16

# The button and the switches are asynchronous; the shells synchronise them.
set_false_path -from [get_ports { RESET dip_sw[*] }]

## Push buttons SW0..SW3
# set_property -dict { PACKAGE_PIN P20 IOSTANDARD LVCMOS33 } [get_ports { btn[0] }] ; # IO_0_14
# set_property -dict { PACKAGE_PIN P19 IOSTANDARD LVCMOS33 } [get_ports { btn[1] }] ; # IO_L5P_T0_D06_14
# set_property -dict { PACKAGE_PIN P17 IOSTANDARD LVCMOS33 } [get_ports { btn[2] }] ; # IO_L21N_T3_DQS_A06_D22_14
# set_property -dict { PACKAGE_PIN N17 IOSTANDARD LVCMOS33 } [get_ports { btn[3] }] ; # IO_L21P_T3_DQS_14

## Seven-segment display, four digits; segments and enables active low
# set_property -dict { PACKAGE_PIN N3 IOSTANDARD LVCMOS33 } [get_ports { seg_en[0] }] ; # 7_SEG1_EN
# set_property -dict { PACKAGE_PIN R1 IOSTANDARD LVCMOS33 } [get_ports { seg_en[1] }] ; # 7_SEG2_EN
# set_property -dict { PACKAGE_PIN P1 IOSTANDARD LVCMOS33 } [get_ports { seg_en[2] }] ; # 7_SEG3_EN
# set_property -dict { PACKAGE_PIN L4 IOSTANDARD LVCMOS33 } [get_ports { seg_en[3] }] ; # 7_SEG4_EN
# set_property -dict { PACKAGE_PIN P4 IOSTANDARD LVCMOS33 } [get_ports { seg[0] }]    ; # 7SEG_0
# set_property -dict { PACKAGE_PIN N4 IOSTANDARD LVCMOS33 } [get_ports { seg[1] }]    ; # 7SEG_1
# set_property -dict { PACKAGE_PIN M3 IOSTANDARD LVCMOS33 } [get_ports { seg[2] }]    ; # 7SEG_2
# set_property -dict { PACKAGE_PIN M5 IOSTANDARD LVCMOS33 } [get_ports { seg[3] }]    ; # 7SEG_3
# set_property -dict { PACKAGE_PIN L5 IOSTANDARD LVCMOS33 } [get_ports { seg[4] }]    ; # 7SEG_4
# set_property -dict { PACKAGE_PIN L6 IOSTANDARD LVCMOS33 } [get_ports { seg[5] }]    ; # 7SEG_5
# set_property -dict { PACKAGE_PIN M6 IOSTANDARD LVCMOS33 } [get_ports { seg[6] }]    ; # 7SEG_6
# set_property -dict { PACKAGE_PIN P5 IOSTANDARD LVCMOS33 } [get_ports { seg[7] }]    ; # 7SEG_7
