{lib, ...}: {
  _module.args.fleetLib = import ../../lib {
    inherit lib;
  };
}
