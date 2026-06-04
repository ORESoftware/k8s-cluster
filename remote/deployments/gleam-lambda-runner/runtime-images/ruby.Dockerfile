FROM docker.io/library/ruby:3.3-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY child-runtimes/ruby-function-runner.rb ./runner.rb
USER 10001:10001
ENTRYPOINT ["ruby", "/opt/dd-lambda/runner.rb"]
