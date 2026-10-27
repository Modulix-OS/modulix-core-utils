{
  framework13 = {
    vendor = "Framework";
    family = "13in Laptop";
    product = "Laptop (11th Gen Intel Core)";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = true;
    usb = [ { id = "27c6"; product = "Goodix Fingerprint Device"; } ];
    lspci = [ "00:02.0 VGA compatible controller: Intel Corporation TigerLake-LP GT2 [Iris Xe Graphics] (rev 01)" ];
    cpuid = "   (synth) = Intel Core i7-1165G7 (Tiger Lake B0) {Willow Cove}, 10nm";
  };

  framework16-amd = {
    vendor = "Framework";
    family = "16in Laptop";
    product = "Laptop 16 (AMD Ryzen 7040 Series)";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = true;
    usb = [ ];
    lspci = [
      "c1:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Phoenix1 (rev c1)"
      "03:00.0 Display controller: Advanced Micro Devices, Inc. [AMD/ATI] Navi 33 [Radeon RX 7700S/7600S/7600M XT/PRO W7600] (rev c1)"
    ];
    cpuid = "   (synth) = AMD Ryzen 7 7840HS (Phoenix A1) {Zen 4}, 4nm";
  };

  thinkpad-t480-hdd = {
    vendor = "LENOVO";
    family = "ThinkPad T480";
    product = "20L5000AUS";
    disks = [ { name = "sda"; ssd = false; } ];
    battery = true;
    iio = false;
    usb = [ { id = "06cb"; product = "Synaptics Fingerprint Reader"; } ];
    lspci = [ "00:02.0 VGA compatible controller: Intel Corporation UHD Graphics 620 (rev 07)" ];
    cpuid = "   (synth) = Intel Core i5-8250U (Kaby Lake-R B0) {Skylake}, 14nm";
  };

  hp-desktop-nvidia = {
    vendor = "Hewlett-Packard";
    family = "103C_53307F HP Z";
    product = "HP Z240 Tower Workstation";
    disks = [ { name = "sda"; ssd = false; } ];
    battery = false;
    iio = false;
    usb = [ ];
    lspci = [ "01:00.0 VGA compatible controller: NVIDIA Corporation GP106 [GeForce GTX 1060 3GB] (rev a1)" ];
    cpuid = "   (synth) = Intel Core i7-6700 (Skylake-S R0) {Skylake}, 14nm";
  };

  asus-rog-hybrid = {
    vendor = "ASUSTeK COMPUTER INC.";
    family = "ROG Zephyrus G14";
    product = "GA401IV";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = false;
    usb = [ ];
    lspci = [
      "06:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Renoir (rev c6)"
      "01:00.0 3D controller: NVIDIA Corporation TU116M [GeForce GTX 1660 Ti Mobile] (rev a1)"
    ];
    cpuid = "   (synth) = AMD Ryzen 9 4900HS (Renoir LCN-A1) {Zen 2}, 7nm";
  };

  qemu-vm = {
    vendor = "QEMU";
    family = "";
    product = "Standard PC (Q35 + ICH9, 2009)";
    disks = [ ];
    battery = false;
    iio = false;
    usb = [ ];
    lspci = [ "00:01.0 VGA compatible controller: Red Hat, Inc. QXL paravirtual graphic card (rev 05)" ];
    cpuid = "   (synth) = AMD EPYC (Milan B0) {Zen 3}, 7nm";
  };

  # Exercises the failure path: no product_family file.
  qemu-no-family = {
    vendor = "QEMU";
    family = null;
    product = "Standard PC (i440FX + PIIX, 1996)";
    disks = [ ];
    battery = false;
    iio = false;
    usb = [ ];
    lspci = [ "00:02.0 VGA compatible controller: Red Hat, Inc. Virtio 1.0 GPU (rev 01)" ];
    cpuid = "   (synth) = Intel Core Processor (Skylake, IBRS) {Skylake}, 14nm";
  };
  surface-pro-7 = {
    vendor = "Microsoft Corporation";
    family = "Surface";
    product = "Surface Pro 7";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = true;
    usb = [ ];
    lspci = [ "00:02.0 VGA compatible controller: Intel Corporation Iris Plus Graphics G4 (Ice Lake) (rev 07)" ];
    cpuid = "   (synth) = Intel Core i5-1035G4 (Ice Lake-U/Y D1) {Sunny Cove}, 10nm";
  };

  surface-laptop-4-amd = {
    vendor = "Microsoft Corporation";
    family = "Surface";
    product = "Surface Laptop 4";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = false;
    usb = [ ];
    lspci = [ "00:01.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Renoir (rev c3)" ];
    cpuid = "   (synth) = AMD Ryzen 5 4680U (Renoir LCN-A1) {Zen 2}, 7nm";
  };

  # Hybrid graphics: Intel iGPU + NVIDIA dGPU exposed as a 3D controller.
  surface-laptop-studio = {
    vendor = "Microsoft Corporation";
    family = "Surface";
    product = "Surface Laptop Studio";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = true;
    usb = [ ];
    lspci = [
      "00:02.0 VGA compatible controller: Intel Corporation TigerLake-H GT1 [UHD Graphics] (rev 01)"
      "01:00.0 3D controller: NVIDIA Corporation GA107M [GeForce RTX 3050 Ti Mobile] (rev a1)"
    ];
    cpuid = "   (synth) = Intel Core i7-11370H (Tiger Lake B0) {Willow Cove}, 10nm";
  };
  surface-pro-6 = {
    vendor = "Microsoft Corporation";
    family = "Surface";
    product = "Surface Pro 6";
    disks = [ { name = "nvme0n1"; ssd = true; } ];
    battery = true;
    iio = true;
    usb = [ ];
    lspci = [ "00:02.0 VGA compatible controller: Intel Corporation UHD Graphics 620 (rev 07)" ];
    cpuid = "   (synth) = Intel Core i5-8350U (Kaby Lake-R Y0) {Skylake}, 14nm";
  };
}
