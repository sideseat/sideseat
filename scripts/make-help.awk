BEGIN {
  FS = ":.*## "
  print "SideSeat repository commands"
  print ""
}

/^[A-Za-z0-9_.%-]+:.*## / {
  printf "  make %-28s %s\n", $1, $2
}
