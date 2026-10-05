# Prints `make help` from the `##@ Section` headings and `target: ## description` annotations in
# every Makefile fragment, in include order.
BEGIN {
  FS = ":.*## "
  print "SideSeat repository commands"
}

/^##@ / {
  printf "\n%s\n", substr($0, 5)
  next
}

/^[A-Za-z0-9_.%-]+:.*## / {
  printf "  make %-28s %s\n", $1, $2
}
