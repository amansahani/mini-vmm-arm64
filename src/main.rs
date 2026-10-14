use applevisor::prelude::*;
use std::thread;

const GUEST_RAM_SIZE: usize = 0x40000; // 256 KB (16 * 16KB pages)
const GUEST_RAM_BASE: u64 = 0x8000_0000; // Common RAM base for ARM64 (QEMU virt / standard ARM layout)
const SERIAL_MMIO_ADDR: u64 = 0x9000_0000; // Mock UART MMIO address

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("==================================================");
    println!("  Mini-VMM (Apple Silicon ARM64 Hypervisor in Rust) ");
    println!("==================================================");

    // 1. Initialize the VM instance
    let vm = VirtualMachine::new()?;
    println!("[1/5] Hypervisor VM instance created via Hypervisor.framework");

    // 2. Allocate and map guest physical memory
    let mut mem = vm.memory_create(GUEST_RAM_SIZE)?;
    mem.map(GUEST_RAM_BASE, MemPerms::ReadWriteExec)?;
    println!(
        "[2/5] Mapped {} KB guest RAM at physical address {:#010x}",
        GUEST_RAM_SIZE / 1024,
        GUEST_RAM_BASE
    );

    // 3. Assemble and load a minimal ARM64 guest program into guest RAM.
    //
    // What this guest assembly does:
    //   - Load the MMIO UART base address (0x0900_0000) into register x0
    //   - Store byte 'H' to [x0]  --> triggers stage-2 MMIO VM Exit on host!
    //   - Store byte 'E' to [x0]  --> triggers VM Exit!
    //   - Store byte 'L' to [x0]  --> triggers VM Exit!
    //   - Store byte 'L' to [x0]  --> triggers VM Exit!
    //   - Store byte 'O' to [x0]  --> triggers VM Exit!
    //   - Store byte ' ' to [x0]  --> triggers VM Exit!
    //   - Store byte 'V' to [x0]  --> triggers VM Exit!
    //   - Store byte 'M' to [x0]  --> triggers VM Exit!
    //   - Store byte '!' to [x0]  --> triggers VM Exit!
    //   - Store byte '\n' to [x0] --> triggers VM Exit!
    //   - brk #0                  --> triggers software breakpoint exception to exit loop
    //
    // Machine code:
    //   d2b20000  movz x0, #0x0900, lsl #16  (x0 = 0x0900_0000)
    //   52800901  movz w1, #'H'
    //   39000001  strb w1, [x0]
    //   528008a1  movz w1, #'E'
    //   39000001  strb w1, [x0]
    //   52800981  movz w1, #'L'
    //   39000001  strb w1, [x0]
    //   52800981  movz w1, #'L'
    //   39000001  strb w1, [x0]
    //   528009e1  movz w1, #'O'
    //   39000001  strb w1, [x0]
    //   52800401  movz w1, #' '
    //   39000001  strb w1, [x0]
    //   52800ac1  movz w1, #'V'
    //   39000001  strb w1, [x0]
    //   528009a1  movz w1, #'M'
    //   39000001  strb w1, [x0]
    //   52800421  movz w1, #'!'
    //   39000001  strb w1, [x0]
    //   52800141  movz w1, #'\n'
    //   39000001  strb w1, [x0]
    //   d4200000  brk #0
    let guest_code: [u32; 22] = [
        0xd2b20000, // movz x0, #0x0900, lsl #16
        0x52800901, // movz w1, #'H'
        0x39000001, // strb w1, [x0]
        0x528008a1, // movz w1, #'E'
        0x39000001, // strb w1, [x0]
        0x52800981, // movz w1, #'L'
        0x39000001, // strb w1, [x0]
        0x52800981, // movz w1, #'L'
        0x39000001, // strb w1, [x0]
        0x528009e1, // movz w1, #'O'
        0x39000001, // strb w1, [x0]
        0x52800401, // movz w1, #' '
        0x39000001, // strb w1, [x0]
        0x52800ac1, // movz w1, #'V'
        0x39000001, // strb w1, [x0]
        0x528009a1, // movz w1, #'M'
        0x39000001, // strb w1, [x0]
        0x52800421, // movz w1, #'!'
        0x39000001, // strb w1, [x0]
        0x52800141, // movz w1, #'\n'
        0x39000001, // strb w1, [x0]
        0xd4200000, // brk #0
    ];

    for (i, &inst) in guest_code.iter().enumerate() {
        mem.write_u32(GUEST_RAM_BASE + (i * 4) as u64, inst)?;
    }
    println!("[3/5] Written 22 ARM64 guest instructions into guest RAM");

    // 4. On macOS Hypervisor.framework, hv_vcpu_create MUST be invoked inside
    // the OS thread that will actually run the vcpu loop.
    let vm_clone = vm.clone();
    let vcpu_thread = thread::spawn(move || {
        let vcpu = vm_clone.vcpu_create().expect("Failed to create vCPU");
        println!("[4/5] Created vCPU id {}", vcpu.id());

        // Set entry point PC
        vcpu.set_reg(Reg::PC, GUEST_RAM_BASE).expect("Failed to set PC");

        // PSTATE (CPSR): EL1h (AArch64 privileged kernel mode) with IRQ/FIQ masked
        // 0x3c5 = M[3:0]=0101 (EL1h), D=1, A=1, I=1, F=1
        vcpu.set_reg(Reg::CPSR, 0x3c5).expect("Failed to set CPSR");

        // Enable trapping debug exceptions (BRK) so they exit to the host hypervisor
        vcpu.set_trap_debug_exceptions(true).expect("Failed to set trap debug exceptions");

        println!("[5/5] Starting vCPU execution loop...\n");
        println!("----------------- GUEST SERIAL CONSOLE OUTPUT -----------------");

        let mut exit_count = 0;

        loop {
            vcpu.run().expect("vCPU run failed");
            let exit_info = vcpu.get_exit_info();
            exit_count += 1;

            match exit_info.reason {
                ExitReason::EXCEPTION => {
                    let syndrome = exit_info.exception.syndrome;
                    let ec = (syndrome >> 26) & 0x3f; // Exception Class

                    // EC 0x24: Data Abort from lower EL
                    // EC 0x25: Data Abort from current EL (EL1)
                    if ec == 0x24 || ec == 0x25 {
                        let fault_addr = exit_info.exception.virtual_address;
                        let phys_addr = exit_info.exception.physical_address;

                        // Check if access hit our Serial UART MMIO region
                        if fault_addr == SERIAL_MMIO_ADDR || phys_addr == SERIAL_MMIO_ADDR {
                            // Extract register index Rt from syndrome bits [20:16]
                            let rt = ((syndrome >> 16) & 0x1f) as u32;
                            // Read register value from vCPU
                            let val = vcpu.get_reg(reg_from_index(rt)).unwrap();
                            let char_out = (val & 0xff) as u8 as char;
                            print!("{}", char_out);

                            // Advance PC past the trapping instruction (4 bytes)
                            let pc = vcpu.get_reg(Reg::PC).unwrap();
                            vcpu.set_reg(Reg::PC, pc + 4).unwrap();
                            continue;
                        }
                    }

                    // EC 0x3c: BRK instruction in AArch64 state (our guest shutdown signal)
                    if ec == 0x3c {
                        println!("---------------------------------------------------------------");
                        println!("\n[+] Guest triggered clean shutdown via BRK instruction (EC=0x3c).");
                        break;
                    }

                    let pc = vcpu.get_reg(Reg::PC).unwrap();
                    eprintln!(
                        "\n[!] Unhandled Exception: EC={:#x}, syndrome={:#x}, PC={:#x}, VA={:#x}, PA={:#x}",
                        ec,
                        syndrome,
                        pc,
                        exit_info.exception.virtual_address,
                        exit_info.exception.physical_address
                    );
                    break;
                }
                ExitReason::CANCELED => {
                    println!("\n[+] vCPU canceled by host.");
                    break;
                }
                ExitReason::VTIMER_ACTIVATED => {
                    // Virtual timer interrupt triggered; in a full OS we would inject IRQ
                    continue;
                }
                ExitReason::UNKNOWN => {
                    eprintln!("\n[!] vCPU exited with UNKNOWN reason");
                    break;
                }
            }
        }

        println!("[+] Total VM exits handled: {}", exit_count);
    });

    vcpu_thread.join().unwrap();
    println!("=== Mini-VMM execution completed successfully ===");
    Ok(())
}

/// Helper to map an ARM64 register index (0..30) to applevisor's Reg enum
fn reg_from_index(rt: u32) -> Reg {
    match rt {
        0 => Reg::X0,
        1 => Reg::X1,
        2 => Reg::X2,
        3 => Reg::X3,
        4 => Reg::X4,
        5 => Reg::X5,
        6 => Reg::X6,
        7 => Reg::X7,
        8 => Reg::X8,
        9 => Reg::X9,
        10 => Reg::X10,
        11 => Reg::X11,
        12 => Reg::X12,
        13 => Reg::X13,
        14 => Reg::X14,
        15 => Reg::X15,
        16 => Reg::X16,
        17 => Reg::X17,
        18 => Reg::X18,
        19 => Reg::X19,
        20 => Reg::X20,
        21 => Reg::X21,
        22 => Reg::X22,
        23 => Reg::X23,
        24 => Reg::X24,
        25 => Reg::X25,
        26 => Reg::X26,
        27 => Reg::X27,
        28 => Reg::X28,
        29 => Reg::X29,
        30 => Reg::X30,
        _ => Reg::X0,
    }
}
