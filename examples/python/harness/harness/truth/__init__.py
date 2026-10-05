"""Ground truth for the message-parsing goldens, derived independently of the parser.

A golden's ``expected.json`` records what the parser reconstructed when the fixture was captured, so a
parser defect present then is blessed. ``truth.json`` instead records what the wire and the scenario
script say the conversation was: :mod:`harness.truth.wire` decodes each recorded model response,
:mod:`harness.truth.derive` lays the responses out as the scenario's conversation, and
:mod:`harness.truth.sources` finds the responses for every producer and scenario.
"""
